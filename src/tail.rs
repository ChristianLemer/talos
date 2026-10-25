//! tail.rs — the last lines a failed step printed, made fit for the journal.
//!
//! A step's output comes out of a PTY: stdout and stderr merged, colours, cursor moves,
//! and `\r` progress bars that repaint one line a hundred times. The journal wants what a
//! person would have read on the screen at the end — so this renders the bytes the way a
//! terminal would have left them, keeps the last few lines, and bounds them.
//!
//! Written only when a step ends `ok: false` (`server::do_step`), and only into the journal,
//! which reaches the share under the user's consent like every other line. Two rewrites on
//! the way: the home folder is written `~` (the journal's rule), and a URL's embedded
//! credentials (`https://user:token@host`, which git prints on a failed clone and some tools
//! print for a proxy) become `***`. Nothing else is masked — a tool that prints a secret in
//! plain text in its error output would be journalled with it. Pure, so it is tested.

use std::sync::LazyLock;

/// How many lines of the end are kept.
pub const MAX_LINES: usize = 20;
/// And at most this many bytes of them, cut at the START on a char boundary.
pub const MAX_BYTES: usize = 2048;

/// A URL's `user:secret@`. The scheme is kept so the line still reads as a URL.
static URL_CREDENTIALS: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b([a-z][a-z0-9+.-]*://)[^/\s:@]+:[^/\s@]+@").unwrap()
});

/// The readable end of `raw`: terminal escapes removed, `\r` repaints collapsed to what
/// stayed on screen, blank lines dropped, the last `MAX_LINES` lines within `MAX_BYTES`,
/// joined by `\n`. `home` (may be empty) is written `~`, without case on Windows paths.
pub fn readable_tail(raw: &str, home: &str) -> String {
    let screen = render(raw);
    let lines: Vec<&str> = screen
        .split('\n')
        .map(|l| l.trim_end())
        .filter(|l| !l.trim().is_empty())
        .collect();
    let start = lines.len().saturating_sub(MAX_LINES);
    let text = lines[start..].join("\n");
    let text = tilde_all_nocase(&text, home);
    let text = URL_CREDENTIALS.replace_all(&text, "${1}***@").into_owned();
    keep_last_bytes(&text, MAX_BYTES).to_string()
}

/// The bytes as a terminal would leave them, line by line. A cursor POSITION (`ESC[r;cH`)
/// starts a new line: ConPTY repaints with those rather than with newlines, and dropping
/// them would glue two lines of the screen into one. Within a line, a `\r` returns to its
/// start, so only what follows the last one is what stayed visible (a progress bar's final
/// state) — a trailing `\r` (the `\r\n` line end) is not a repaint.
fn render(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len().min(64 * 1024));
    let mut line = String::new();
    let mut chars = raw.chars().peekable();
    let flush = |line: &mut String, out: &mut String| {
        out.push_str(line);
        out.push('\n');
        line.clear();
    };
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => match chars.next() {
                // CSI: parameters, then one final byte in @..~.
                Some('[') => {
                    let mut fin = None;
                    for n in chars.by_ref() {
                        if ('@'..='~').contains(&n) {
                            fin = Some(n);
                            break;
                        }
                    }
                    if matches!(fin, Some('H' | 'f')) {
                        flush(&mut line, &mut out);
                    }
                }
                // OSC (a window title, a hyperlink): up to BEL or ESC \.
                Some(']') => {
                    while let Some(n) = chars.next() {
                        if n == '\x07' {
                            break;
                        }
                        if n == '\x1b' {
                            chars.next_if_eq(&'\\');
                            break;
                        }
                    }
                }
                // A charset designation carries one more byte (`ESC ( B`).
                Some('(' | ')' | '*' | '+') => {
                    chars.next();
                }
                // Any other two-byte escape (`ESC =`, `ESC 7`…).
                _ => {}
            },
            '\n' => flush(&mut line, &mut out),
            '\r' => {
                // `\r\n` ends the line; a lone `\r` repaints it.
                if chars.peek() != Some(&'\n') {
                    line.clear();
                }
            }
            '\t' => line.push(c),
            c if c.is_control() => {}
            c => line.push(c),
        }
    }
    out.push_str(&line);
    out
}

/// `machine::tilde_all`, without case: a tool prints `C:\Users\Alice` or `c:\users\alice`
/// as it likes, and with `/` as often as `\`. Both spellings of a Windows home are written
/// `~`. A POSIX home keeps its case, as its filesystem does.
fn tilde_all_nocase(text: &str, home: &str) -> String {
    if home.is_empty() {
        return text.to_string();
    }
    let windows = home.contains('\\') || home.as_bytes().get(1) == Some(&b':');
    if !windows {
        return crate::machine::tilde_all(text, home);
    }
    let mut out = text.to_string();
    for spelling in [home.replace('/', "\\"), home.replace('\\', "/")] {
        let pattern = format!("(?i){}", regex::escape(&spelling));
        if let Ok(re) = regex::Regex::new(&pattern) {
            out = re.replace_all(&out, "~").into_owned();
        }
    }
    out
}

/// The last `max` bytes of `s`, starting on a char boundary.
fn keep_last_bytes(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut start = s.len() - max;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    &s[start..]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_and_titles_are_removed() {
        let raw = "\x1b]0;Windows PowerShell\x07\x1b[31mError:\x1b[0m unknown flag --force\r\n";
        assert_eq!(readable_tail(raw, ""), "Error: unknown flag --force");
    }

    #[test]
    fn a_progress_bar_keeps_only_what_stayed_on_screen() {
        let raw = "Downloading\r\n  10%\r  55%\r 100%\r\nInstaller failed with exit code: 1603\r\n";
        assert_eq!(
            readable_tail(raw, ""),
            "Downloading\n 100%\nInstaller failed with exit code: 1603"
        );
    }

    #[test]
    fn a_conpty_cursor_position_is_a_new_line_not_glue() {
        // ConPTY repaints with cursor positions; without the break the two would read
        // "Found Orca [x]No applicable upgrade found."
        let raw = "\x1b[?25l\x1b[2J\x1b[HFound Orca [x]\x1b[K\x1b[2;1HNo applicable upgrade found.\x1b[K\x1b[?25h";
        assert_eq!(
            readable_tail(raw, ""),
            "Found Orca [x]\nNo applicable upgrade found."
        );
    }

    #[test]
    fn only_the_last_lines_are_kept_and_blank_ones_do_not_count() {
        let raw: String = (1..=50).map(|n| format!("line {n}\r\n\r\n")).collect();
        let t = readable_tail(&raw, "");
        assert_eq!(t.lines().count(), MAX_LINES);
        assert!(t.starts_with("line 31\n") && t.ends_with("line 50"), "{t}");
    }

    #[test]
    fn the_bytes_are_bounded_on_a_char_boundary_and_the_end_is_kept() {
        let raw = format!("{}\nthe end", "é".repeat(5000));
        let t = readable_tail(&raw, "");
        assert!(t.len() <= MAX_BYTES, "{}", t.len());
        assert!(t.ends_with("the end"));
        // Slicing would have panicked mid-char; reaching here is half the proof.
        assert!(t.starts_with('é'));
    }

    #[test]
    fn the_tail_stays_one_journal_line() {
        let t = readable_tail("a\r\nb\r\n", "");
        let line = serde_json::to_string(&serde_json::json!({ "tail": t })).unwrap();
        assert!(!line.contains('\n'), "{line}");
    }

    #[test]
    fn the_home_folder_is_written_as_a_tilde_whatever_its_case_or_slashes() {
        let raw = "Log: c:\\users\\ALICE\\AppData\\Local\\Temp\\x.log\r\nsee C:/Users/alice/x\r\n";
        assert_eq!(
            readable_tail(raw, "C:\\Users\\alice"),
            "Log: ~\\AppData\\Local\\Temp\\x.log\nsee ~/x"
        );
        assert_eq!(
            readable_tail("open /home/alice/.npmrc\n", "/home/alice"),
            "open ~/.npmrc"
        );
    }

    #[test]
    fn credentials_in_a_url_are_masked_and_the_url_still_reads() {
        let raw = "fatal: unable to access 'https://bob:ghp_abc123@github.com/x/y.git/'\n\
                   proxy http://DOMAIN%5Cbob:hunter2@proxy.corp:8080 refused\n\
                   plain https://github.com/x/y stays\n";
        assert_eq!(
            readable_tail(raw, ""),
            "fatal: unable to access 'https://***@github.com/x/y.git/'\n\
             proxy http://***@proxy.corp:8080 refused\n\
             plain https://github.com/x/y stays"
        );
    }

    #[test]
    fn an_empty_output_has_an_empty_tail() {
        assert_eq!(readable_tail("", "/home/a"), "");
        assert_eq!(readable_tail("\r\n\x1b[0m\r\n", "/home/a"), "");
    }
}
