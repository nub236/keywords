use clap::{Parser, Subcommand};
use once_cell::sync::Lazy;
use rphonetic::{DoubleMetaphone, Encoder};
use serde::{Deserialize, Serialize};
use std::io::{self, IsTerminal, Read, Write};
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Matcher (ported from Handy src-tauri/src/audio_toolkit/text.rs)
// ---------------------------------------------------------------------------

static DOUBLE_METAPHONE: Lazy<DoubleMetaphone> =
    Lazy::new(|| DoubleMetaphone::new(Some(64)));

const MIN_PHONETIC_CODE_LEN: usize = 3;
const STRONG_PHONETIC_CODE_LEN: usize = 5;
const PHONETIC_SCORE_WEIGHT: f64 = 0.3;
const STRONG_PHONETIC_SCORE_WEIGHT: f64 = 0.2;
const DEFAULT_THRESHOLD: f64 = 0.18;

#[derive(Clone, Debug, PartialEq, Eq)]
struct PhoneticCode {
    primary: String,
    alternate: String,
}

impl PhoneticCode {
    fn encode(word: &str) -> Option<Self> {
        if !supports_phonetics(word) {
            return None;
        }
        let primary = DOUBLE_METAPHONE.encode(word);
        let alternate = DOUBLE_METAPHONE.encode_alternate(word);
        if primary.chars().count() < MIN_PHONETIC_CODE_LEN {
            return None;
        }
        Some(Self { primary, alternate })
    }

    fn matches(&self, other: &Self) -> bool {
        self.primary == other.primary
            || self.primary == other.alternate
            || self.alternate == other.primary
            || self.alternate == other.alternate
    }

    fn len(&self) -> usize {
        self.primary.chars().count()
    }
}

fn build_ngram(words: &[&str]) -> String {
    words
        .iter()
        .map(|w| build_match_key(w))
        .collect::<Vec<_>>()
        .concat()
}

fn build_match_key(word: &str) -> String {
    word.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

struct CustomWordMatchKey {
    word_index: usize,
    key: String,
    phonetic: Option<PhoneticCode>,
}

impl CustomWordMatchKey {
    fn new(word_index: usize, key: String) -> Self {
        let phonetic = PhoneticCode::encode(&key);
        Self {
            word_index,
            key,
            phonetic,
        }
    }
}

fn build_custom_word_match_keys(word: &str, word_index: usize) -> Vec<CustomWordMatchKey> {
    let primary_key = build_match_key(word);
    let mut keys = Vec::with_capacity(2);

    if is_supported_fuzzy_key(&primary_key) {
        keys.push(CustomWordMatchKey::new(word_index, primary_key.clone()));
    }

    if word.contains('&') {
        let expanded_key = build_match_key(&word.replace('&', " and "));
        if is_supported_fuzzy_key(&expanded_key) && expanded_key != primary_key {
            keys.push(CustomWordMatchKey::new(word_index, expanded_key));
        }
    }

    keys
}

fn is_supported_fuzzy_key(key: &str) -> bool {
    !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric())
}

fn supports_phonetics(key: &str) -> bool {
    !key.is_empty() && key.chars().all(|c| c.is_ascii_alphabetic())
}

fn find_best_match<'a>(
    candidate: &str,
    custom_words: &'a [String],
    custom_word_match_keys: &[CustomWordMatchKey],
    threshold: f64,
) -> Option<(&'a String, f64)> {
    if !is_supported_fuzzy_key(candidate) || candidate.chars().count() > 50 {
        return None;
    }

    let mut best_match: Option<&String> = None;
    let mut best_score = f64::MAX;

    let candidate_phonetic = PhoneticCode::encode(candidate);

    for custom_word_key in custom_word_match_keys {
        let phonetic_match = match (&candidate_phonetic, &custom_word_key.phonetic) {
            (Some(candidate_code), Some(key_code)) => candidate_code.matches(key_code),
            _ => false,
        };

        let candidate_len = candidate.chars().count();
        let custom_word_len = custom_word_key.key.chars().count();
        let len_diff = candidate_len.abs_diff(custom_word_len) as f64;
        let max_len = candidate_len.max(custom_word_len) as f64;
        let len_ratio = if phonetic_match { 0.5 } else { 0.25 };
        let max_allowed_diff = (max_len * len_ratio).max(2.0);
        if len_diff > max_allowed_diff {
            continue;
        }

        let levenshtein_dist = strsim::levenshtein(candidate, &custom_word_key.key);
        let levenshtein_score = if max_len > 0.0 {
            levenshtein_dist as f64 / max_len
        } else {
            1.0
        };

        let combined_score = if phonetic_match {
            let strong = candidate_phonetic
                .as_ref()
                .is_some_and(|code| code.len() >= STRONG_PHONETIC_CODE_LEN);
            let weight = if strong {
                STRONG_PHONETIC_SCORE_WEIGHT
            } else {
                PHONETIC_SCORE_WEIGHT
            };
            levenshtein_score * weight
        } else {
            levenshtein_score
        };

        if combined_score < threshold && combined_score < best_score {
            best_match = Some(&custom_words[custom_word_key.word_index]);
            best_score = combined_score;
        }
    }

    best_match.map(|m| (m, best_score))
}

pub fn apply_custom_words(text: &str, custom_words: &[String], threshold: f64) -> String {
    if custom_words.is_empty() {
        return text.to_string();
    }

    let custom_word_match_keys: Vec<CustomWordMatchKey> = custom_words
        .iter()
        .enumerate()
        .flat_map(|(index, word)| build_custom_word_match_keys(word, index))
        .collect();

    let words: Vec<&str> = text.split_whitespace().collect();
    let mut result = Vec::new();
    let mut i = 0;

    while i < words.len() {
        let mut best_match: Option<(usize, &String, f64)> = None;

        for n in (1..=3).rev() {
            if i + n > words.len() {
                continue;
            }

            let ngram_words = &words[i..i + n];
            if ngram_words[..n.saturating_sub(1)]
                .iter()
                .any(|word| !extract_punctuation(word).1.is_empty())
            {
                continue;
            }
            let ngram = build_ngram(ngram_words);

            if let Some((replacement, score)) =
                find_best_match(&ngram, custom_words, &custom_word_match_keys, threshold)
            {
                let is_better = best_match
                    .as_ref()
                    .is_none_or(|(_, _, best_score)| score < *best_score);
                if is_better {
                    best_match = Some((n, replacement, score));
                }
            }
        }

        if let Some((n, replacement, _)) = best_match {
            let ngram_words = &words[i..i + n];
            let (prefix, _) = extract_punctuation(ngram_words[0]);
            let (_, suffix) = extract_punctuation(ngram_words[n - 1]);

            let corrected = preserve_case_pattern_for_span(ngram_words, replacement);

            result.push(format!("{}{}{}", prefix, corrected, suffix));
            i += n;
        } else {
            let rewritten = apply_custom_words_within_token(
                words[i],
                custom_words,
                &custom_word_match_keys,
                threshold,
            );
            result.push(rewritten.unwrap_or_else(|| words[i].to_string()));
            i += 1;
        }
    }

    result.join(" ")
}

fn has_intentional_case(replacement: &str) -> bool {
    let mut chars = replacement.chars();
    let Some(first) = chars.next() else {
        return false;
    };

    chars.any(|c| c.is_uppercase()) && (first.is_lowercase() || chars.any(|c| c.is_lowercase()))
}

fn is_shouted(words: &[&str]) -> bool {
    let letters: usize = words
        .iter()
        .flat_map(|word| word.chars())
        .filter(|c| c.is_alphabetic())
        .count();

    letters > 1
        && words
            .iter()
            .flat_map(|word| word.chars())
            .filter(|c| c.is_alphabetic())
            .all(char::is_uppercase)
}

fn preserve_case_pattern_for_span(original: &[&str], replacement: &str) -> String {
    let first = original.first().copied().unwrap_or("");

    if is_shouted(original) {
        replacement.to_uppercase()
    } else if has_intentional_case(replacement) {
        replacement.to_string()
    } else if first.chars().next().is_some_and(|c| c.is_uppercase()) {
        let mut chars: Vec<char> = replacement.chars().collect();
        if let Some(first_char) = chars.get_mut(0) {
            *first_char = first_char.to_uppercase().next().unwrap_or(*first_char);
        }
        chars.into_iter().collect()
    } else {
        replacement.to_string()
    }
}

fn split_token_segments(token: &str) -> Vec<String> {
    let mut segments: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_is_alnum = true;

    for c in token.chars() {
        let is_alnum = c.is_alphanumeric();
        if is_alnum != current_is_alnum && !current.is_empty() {
            segments.push(std::mem::take(&mut current));
            current_is_alnum = is_alnum;
        } else if current.is_empty() {
            current_is_alnum = is_alnum;
        }
        current.push(c);
    }

    if !current.is_empty() {
        segments.push(current);
    }

    if segments
        .first()
        .is_some_and(|segment| !segment.chars().next().is_some_and(char::is_alphanumeric))
    {
        segments.insert(0, String::new());
    }

    segments
}

fn apply_custom_words_within_token(
    token: &str,
    custom_words: &[String],
    custom_word_match_keys: &[CustomWordMatchKey],
    threshold: f64,
) -> Option<String> {
    let mut segments = split_token_segments(token);
    if segments.len() < 3 {
        return None;
    }

    let mut matched = false;
    for index in (0..segments.len()).step_by(2) {
        let run = &segments[index].clone();
        let key = build_match_key(run);
        if key.is_empty() {
            continue;
        }

        if let Some((replacement, _)) =
            find_best_match(&key, custom_words, &custom_word_match_keys, threshold)
        {
            segments[index] =
                preserve_case_pattern_for_span(&[run.as_str()], replacement);
            matched = true;
        }
    }

    matched.then(|| segments.concat())
}

fn extract_punctuation(word: &str) -> (&str, &str) {
    let prefix_end = word
        .char_indices()
        .find(|(_, c)| c.is_alphanumeric())
        .map(|(index, _)| index)
        .unwrap_or(word.len());
    let suffix_start = word
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_alphanumeric())
        .map(|(index, c)| index + c.len_utf8())
        .unwrap_or(0);

    let prefix = if prefix_end > 0 {
        &word[..prefix_end]
    } else {
        ""
    };

    let suffix = if suffix_start < word.len() {
        &word[suffix_start..]
    } else {
        ""
    };

    (prefix, suffix)
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
struct Store {
    #[serde(default)]
    keywords: Vec<String>,
    #[serde(default = "default_threshold")]
    threshold: f64,
}

fn default_threshold() -> f64 {
    DEFAULT_THRESHOLD
}

impl Default for Store {
    fn default() -> Self {
        Self {
            keywords: Vec::new(),
            threshold: DEFAULT_THRESHOLD,
        }
    }
}

fn config_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("keywords");
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".config").join("keywords")
}

fn store_path_override() -> Option<PathBuf> {
    std::env::var("KEYWORDS_FILE")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}

fn store_path(cli_file: Option<&str>) -> PathBuf {
    if let Some(f) = cli_file {
        return PathBuf::from(f);
    }
    if let Some(p) = store_path_override() {
        return p;
    }
    config_dir().join("keywords.json")
}

fn load_store(path: &PathBuf) -> Store {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Store::default();
    };
    // Back-compat: allow the file to be a bare JSON array of strings.
    if let Ok(kws) = serde_json::from_str::<Vec<String>>(&raw) {
        return Store {
            keywords: kws,
            threshold: DEFAULT_THRESHOLD,
        };
    }
    serde_json::from_str::<Store>(&raw).unwrap_or_default()
}

fn save_store(path: &PathBuf, store: &Store) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(store).unwrap())?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn effective_threshold(store: &Store, flag: Option<f64>) -> f64 {
    if let Some(t) = flag {
        return t;
    }
    if let Ok(env) = std::env::var("KEYWORDS_THRESHOLD") {
        if let Ok(t) = env.parse::<f64>() {
            return t;
        }
    }
    if store.threshold > 0.0 && store.threshold < 1.0 {
        store.threshold
    } else {
        DEFAULT_THRESHOLD
    }
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(name = "keywords", version, about = "Fuzzy keyword correction for Spokenly (stdin -> stdout)")]
struct Cli {
    /// Alternate keywords file (default: ~/.config/keywords/keywords.json, or $KEYWORDS_FILE)
    #[arg(long, global = true)]
    file: Option<String>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Add one or more keywords (e.g. keywords add "ChargeBee" "FlitePath")
    Add {
        /// Keywords to add
        keyword: Vec<String>,
    },
    /// Remove one or more keywords (case-insensitive)
    Remove {
        /// Keywords to remove
        keyword: Vec<String>,
    },
    /// List keywords
    List {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Remove all keywords
    Clear {},
    /// Show or set the match threshold (default 0.18; lower = stricter)
    Threshold {
        /// New threshold value, e.g. 0.18
        value: Option<f64>,
    },
    /// Correct text from stdin/args with fuzzy keyword matching (this is what Spokenly calls)
    Fix {
        /// Text to correct. If omitted, reads stdin (Spokenly mode).
        text: Vec<String>,
        /// Override threshold for this run
        #[arg(long)]
        threshold: Option<f64>,
    },
    /// Dry-run: show input -> output without Spokenly pipe semantics
    Test {
        /// Text to correct. If omitted, reads stdin.
        text: Vec<String>,
        /// Override threshold for this run
        #[arg(long)]
        threshold: Option<f64>,
    },
    /// Interactive manager: list / add / remove / test keywords
    Manager {},
    /// Print the keywords file path
    #[command(name = "config-path")]
    ConfigPath {},
}

fn print_stdout_exact(s: &str) {
    // Spokenly captures stdout verbatim: no trailing newline when piped.
    // When talking to a terminal, add one for ergonomics.
    if io::stdout().is_terminal() {
        println!("{s}");
    } else {
        print!("{s}");
        let _ = io::stdout().flush();
    }
}

fn read_stdin_all() -> String {
    let mut buf = String::new();
    let _ = io::stdin().read_to_string(&mut buf);
    buf
}

fn run_fix(store: &Store, text_parts: &[String], flag_threshold: Option<f64>, dry_run: bool) -> i32 {
    let input = if text_parts.is_empty() {
        read_stdin_all()
    } else {
        text_parts.join(" ")
    };
    let threshold = effective_threshold(store, flag_threshold);
    let out = apply_custom_words(&input, &store.keywords, threshold);
    if dry_run {
        println!("in:  {input}");
        println!("out: {out}");
    } else {
        print_stdout_exact(&out);
    }
    0
}

fn run_manager(path: &PathBuf) -> i32 {
    println!("Keyword manager ({})", path.display());
    println!("Commands: list | add <word> | remove <word> | test <text> | threshold [value] | quit");
    let stdin = io::stdin();
    let mut line = String::new();
    loop {
        let mut store = load_store(path);
        print!("> ");
        let _ = io::stdout().flush();
        line.clear();
        if stdin.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let mut parts = trimmed.split_whitespace();
        let cmd = parts.next().unwrap_or("").to_lowercase();
        let rest: Vec<&str> = parts.collect();
        match cmd.as_str() {
            "quit" | "exit" | "q" => break,
            "list" | "ls" => {
                if store.keywords.is_empty() {
                    println!("(no keywords yet)");
                } else {
                    for (i, kw) in store.keywords.iter().enumerate() {
                        println!("{}. {kw}", i + 1);
                    }
                }
                println!("threshold: {}", store.threshold);
            }
            "add" => {
                let joined = rest.join(" ");
                if joined.is_empty() {
                    println!("usage: add <keyword>");
                    continue;
                }
                if store.keywords.iter().any(|k| k == &joined) {
                    println!("already have {joined:?}");
                } else {
                    store.keywords.push(joined.clone());
                    if let Err(e) = save_store(path, &store) {
                        eprintln!("save failed: {e}");
                    } else {
                        println!("added {joined:?}");
                    }
                }
            }
            "remove" | "rm" | "delete" => {
                let joined = rest.join(" ");
                if joined.is_empty() {
                    println!("usage: remove <keyword>");
                    continue;
                }
                let before = store.keywords.len();
                store
                    .keywords
                    .retain(|k| k.to_lowercase() != joined.to_lowercase());
                if store.keywords.len() == before {
                    println!("not found: {joined:?}");
                } else if let Err(e) = save_store(path, &store) {
                    eprintln!("save failed: {e}");
                } else {
                    println!("removed {joined:?}");
                }
            }
            "test" => {
                let text = rest.join(" ");
                if text.is_empty() {
                    println!("usage: test <text>");
                    continue;
                }
                let out = apply_custom_words(&text, &store.keywords, store.threshold);
                println!("in:  {text}");
                println!("out: {out}");
            }
            "threshold" => {
                if rest.is_empty() {
                    println!("threshold: {}", store.threshold);
                } else if let Ok(t) = rest[0].parse::<f64>() {
                    if t <= 0.0 || t >= 1.0 {
                        println!("threshold must be between 0 and 1 (e.g. 0.18)");
                    } else {
                        store.threshold = t;
                        if let Err(e) = save_store(path, &store) {
                            eprintln!("save failed: {e}");
                        } else {
                            println!("threshold set to {t}");
                        }
                    }
                } else {
                    println!("usage: threshold [value]");
                }
            }
            "help" => {
                println!("Commands: list | add <word> | remove <word> | test <text> | threshold [value] | quit");
            }
            _ => {
                println!("unknown command {cmd:?} — try: list, add, remove, test, threshold, quit");
            }
        }
    }
    0
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    let path = store_path(cli.file.as_deref());

    let code = match cli.command {
        None => {
            // No subcommand: behave like `fix` so `echo hi | keywords` just works.
            // But if stdin is a terminal (user just typed `keywords`), show help instead of hanging.
            if io::stdin().is_terminal() {
                use clap::CommandFactory;
                let _ = Cli::command().print_help();
                println!();
                0
            } else {
                let store = load_store(&path);
                run_fix(&store, &[], None, false)
            }
        }
        Some(Command::Add { keyword }) => {
            if keyword.is_empty() {
                eprintln!("usage: keywords add <keyword> [...]");
                2
            } else {
                let mut store = load_store(&path);
                let mut added = 0;
                for kw in keyword {
                    if store.keywords.iter().any(|k| k == &kw) {
                        println!("already have {kw:?}");
                    } else {
                        store.keywords.push(kw.clone());
                        println!("added {kw:?}");
                        added += 1;
                    }
                }
                if added > 0 {
                    if let Err(e) = save_store(&path, &store) {
                        eprintln!("save failed: {e}");
                        return std::process::ExitCode::from(1);
                    }
                }
                0
            }
        }
        Some(Command::Remove { keyword }) => {
            if keyword.is_empty() {
                eprintln!("usage: keywords remove <keyword> [...]");
                2
            } else {
                let mut store = load_store(&path);
                for kw in keyword {
                    let before = store.keywords.len();
                    store.keywords.retain(|k| k.to_lowercase() != kw.to_lowercase());
                    if store.keywords.len() == before {
                        println!("not found: {kw:?}");
                    } else {
                        println!("removed {kw:?}");
                    }
                }
                if let Err(e) = save_store(&path, &store) {
                    eprintln!("save failed: {e}");
                    return std::process::ExitCode::from(1);
                }
                0
            }
        }
        Some(Command::List { json }) => {
            let store = load_store(&path);
            if json {
                println!("{}", serde_json::to_string_pretty(&store.keywords).unwrap());
            } else if store.keywords.is_empty() {
                println!("(no keywords yet — add one with: keywords add \"MyTerm\")");
            } else {
                for kw in &store.keywords {
                    println!("{kw}");
                }
            }
            0
        }
        Some(Command::Clear {}) => {
            let store = Store {
                keywords: Vec::new(),
                threshold: load_store(&path).threshold,
            };
            if let Err(e) = save_store(&path, &store) {
                eprintln!("save failed: {e}");
                1
            } else {
                println!("cleared all keywords");
                0
            }
        }
        Some(Command::Threshold { value }) => {
            let mut store = load_store(&path);
            match value {
                None => {
                    println!("{}", store.threshold);
                    0
                }
                Some(t) => {
                    if t <= 0.0 || t >= 1.0 {
                        eprintln!("threshold must be between 0 and 1 (e.g. 0.18; lower = stricter)");
                        2
                    } else {
                        store.threshold = t;
                        if let Err(e) = save_store(&path, &store) {
                            eprintln!("save failed: {e}");
                            1
                        } else {
                            println!("threshold set to {t}");
                            0
                        }
                    }
                }
            }
        }
        Some(Command::Fix { text, threshold }) => {
            let store = load_store(&path);
            run_fix(&store, &text, threshold, false)
        }
        Some(Command::Test { text, threshold }) => {
            let store = load_store(&path);
            run_fix(&store, &text, threshold, true)
        }
        Some(Command::Manager {}) => run_manager(&path),
        Some(Command::ConfigPath {}) => {
            println!("{}", path.display());
            0
        }
    };

    std::process::ExitCode::from(code as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(words: &[&str]) -> Vec<String> {
        words.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn silent_gh_compound_matches() {
        let custom = w(&["FlitePath"]);
        assert_eq!(apply_custom_words("flight path", &custom, 0.18), "FlitePath");
    }

    #[test]
    fn split_words_join() {
        let custom = w(&["ChargeBee"]);
        let out = apply_custom_words("il cui nome è Charge B, che permette", &custom, 0.5);
        assert!(out.contains("ChargeBee,"));
    }

    #[test]
    fn chatgpt_joins() {
        let custom = w(&["ChatGPT"]);
        let out = apply_custom_words("use Chat G P T for this", &custom, 0.5);
        assert!(out.contains("ChatGPT"));
    }

    #[test]
    fn no_swallow_ordinary_words() {
        let custom = w(&["FlitePath", "HiTek"]);
        for text in ["flat pack", "the fleet", "flight deck", "high tea"] {
            assert_eq!(apply_custom_words(text, &custom, 0.18), text);
        }
    }

    #[test]
    fn compound_domain_keeps_suffix() {
        let custom = w(&["FlitePath"]);
        assert_eq!(
            apply_custom_words("visit flightpath.com today", &custom, 0.18),
            "visit FlitePath.com today"
        );
    }

    #[test]
    fn entry_casing_preserved() {
        let custom = w(&["FlitePath"]);
        assert_eq!(apply_custom_words("Flight path", &custom, 0.18), "FlitePath");
        assert_eq!(
            apply_custom_words("FLIGHT PATH", &custom, 0.18),
            "FLITEPATH"
        );
    }
}
