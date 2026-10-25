//! tail.rs — the last lines a failed step printed, made fit for the journal.
//!
//! A step's output comes out of a PTY: stdout and stderr merged, colours, `\r` progress bars,
//! and — under ConPTY — a screen REPAINTED with cursor moves, runs of spaces sent as "cursor
//! forward", a progress bar redrawn cell by cell at an absolute position. Reading that as text
//! glues words and multiplies fragments; hand-rolled rules for it each missed a case. So the
//! bytes are fed to a real terminal emulator (`vt100`) of the row terminal's own size, and
//! the journal keeps what that screen and its scrollback show at the end: what a person would
//! have read.
//!
//! Written only when a step ends `ok: false` (`server::do_step`), and only into the journal,
//! which reaches the share under the user's consent like every other line. On the way:
//! every user-profile path is written `~` (the home folder, a short `CHRIST~1` form, another
//! profile, an MSYS `/c/Users/…`), a URL's credentials become `***@`, and the answers Talos
//! itself typed into the step (a sudo password) become `***`. Nothing else is masked — a tool
//! that prints a secret in plain text in its error output, or a user name outside a path,
//! would be journalled with it. Pure, so it is tested.

use std::sync::LazyLock;

/// How many lines of the end are kept.
pub const MAX_LINES: usize = 20;
/// And at most this many bytes of text, cut at the START on a char boundary. (JSON escaping
/// on the way to the file can make the line longer: a `\` is written twice.)
pub const MAX_BYTES: usize = 2048;
/// Only the end of the output is rendered: a build that printed megabytes still yields its
/// last lines from this much, and the cost stays flat. Cut on a char boundary.
const RENDER_LAST_BYTES: usize = 256 * 1024;
/// The row terminal's geometry, the one `pty::run` opens: ConPTY positions its cursor in
/// THAT screen, so the emulator must have the same one to put each cell back where it was.
const ROWS: u16 = crate::pty::ROW_TERMINAL_ROWS;
const COLS: u16 = crate::pty::ROW_TERMINAL_COLS;
/// Lines kept above the screen. Enough for `MAX_LINES` even when many are blank.
const SCROLLBACK: usize = 1000;

/// A URL's userinfo, `user:secret@` or a token alone (`https://ghp_x@github.com`). The scheme
/// is kept so the line still reads as a URL.
static URL_CREDENTIALS: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)\b([a-z][a-z0-9+.-]*://)[^/\s@]+@").unwrap());

/// A user-profile folder, whoever's and however spelled: `C:\Users\<name>` (the 8.3
/// `CHRIST~1` included), `C:/Users/<name>`, MSYS `/c/Users/<name>`, `/home/<name>`,
/// `/Users/<name>`. It must START a path — `api.github.com/users/x` and `/srv/myhome` are
/// not profiles — and the name runs to the next separator or to what cannot be in a path; a
/// space is part of it only when a separator follows (`replace_profiles` decides).
static PROFILE_PATH: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"(?i)(^|[^\w.~-])((?:[a-z]:|/[a-z])?[\\/](?:users|home)[\\/])([^\\/\r\n"'<>|:;,()\[\]]+)"#,
    )
    .unwrap()
});

/// Folders under `Users` or `home` that are nobody's profile. Masking them would send a
/// reader to a per-user path that does not exist.
const NOT_A_PROFILE: &[&str] = &[
    "public",
    "default",
    "default user",
    "all users",
    "linuxbrew",
];

/// The readable end of `raw`: the screen a terminal would have left, its last `MAX_LINES`
/// non-blank lines within `MAX_BYTES`, joined by `\n`. `home` (may be empty) is written `~`;
/// each of `typed` (what Talos answered into the step) is written `***`.
pub fn readable_tail(raw: &str, home: &str, typed: &[&str]) -> String {
    // Masked in the raw bytes AND in the rendered text: the terminal may render an answer
    // differently from what was typed (a trailing space trimmed off the line).
    let mut raw = keep_last_bytes(raw, RENDER_LAST_BYTES).to_string();
    for secret in typed.iter().filter(|s| !s.is_empty()) {
        raw = raw.replace(secret, "***");
    }
    // A third-party parser fed a tool's arbitrary bytes: should it ever panic, the journal
    // line is still written, without its tail — never an Apply lost mid-run.
    let lines = std::panic::catch_unwind(|| screen_lines(&raw)).unwrap_or_default();
    let start = lines.len().saturating_sub(MAX_LINES);
    let mut text = lines[start..].join("\n");
    for secret in typed.iter().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        text = text.replace(secret, "***");
    }
    let text = tilde_home(&text, home);
    let text = replace_profiles(&text);
    let text = URL_CREDENTIALS.replace_all(&text, "${1}***@");
    keep_last_bytes(&text, MAX_BYTES).to_string()
}

/// Every non-blank line the emulated terminal ends with — scrollback first, then the screen
/// — a row the terminal wrapped joined back to the next one.
fn screen_lines(raw: &str) -> Vec<String> {
    let mut parser = vt100::Parser::new(ROWS, COLS, SCROLLBACK);
    parser.process(raw.as_bytes());
    // A full-screen tool that died in its alternate screen leaves the main one — where the
    // error was printed — hidden behind its last frame.
    if parser.screen().alternate_screen() {
        parser.process(b"\x1b[?1049l");
    }
    let screen = parser.screen_mut();
    // `set_scrollback(k)` shows the rows `k` above the screen; it clamps to what exists.
    screen.set_scrollback(usize::MAX);
    let above = screen.scrollback();
    let mut rows: Vec<(String, bool)> = Vec::with_capacity(above + ROWS as usize);
    for k in (1..=above).rev() {
        screen.set_scrollback(k);
        let text = screen.rows(0, COLS).next().unwrap_or_default();
        rows.push((text, screen.row_wrapped(0)));
    }
    screen.set_scrollback(0);
    for (r, text) in screen.rows(0, COLS).enumerate() {
        rows.push((text, screen.row_wrapped(r as u16)));
    }
    let mut out = Vec::new();
    let mut current = String::new();
    for (text, wrapped) in rows {
        current.push_str(&text);
        if !wrapped {
            let line = current.trim_end();
            if !line.trim().is_empty() {
                out.push(line.to_string());
            }
            current.clear();
        }
    }
    let line = current.trim_end();
    if !line.trim().is_empty() {
        out.push(line.to_string());
    }
    out
}

/// Every profile path written `~` (see `PROFILE_PATH`), what precedes it kept.
fn replace_profiles(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for c in PROFILE_PATH.captures_iter(text) {
        let (Some(lead), Some(name)) = (c.get(1), c.get(3)) else {
            continue;
        };
        // A name runs across a space only into a path: `C:\Users\John Smith\AppData`, but
        // not `/home/alice is missing`.
        let into_path = text[name.end()..].starts_with(['\\', '/']);
        let name_end = if into_path {
            name.end()
        } else {
            name.start()
                + name
                    .as_str()
                    .find(char::is_whitespace)
                    .unwrap_or(name.len())
        };
        let own = text[name.start()..name_end].trim().to_lowercase();
        out.push_str(&text[last..lead.end()]);
        if NOT_A_PROFILE.contains(&own.as_str()) {
            out.push_str(&text[lead.end()..name_end]);
        } else {
            out.push('~');
        }
        last = name_end;
    }
    out.push_str(&text[last..]);
    out
}

/// The home folder written `~`, wherever it appears. Without case for a Windows home, in
/// either slash; never inside a longer name (`/home/al` is not the start of `/home/alice2`).
fn tilde_home(text: &str, home: &str) -> String {
    let home = home.trim_end_matches(['/', '\\']);
    if home.is_empty() {
        return text.to_string();
    }
    let windows = home.contains('\\') || home.as_bytes().get(1) == Some(&b':');
    let spellings = if windows {
        vec![home.replace('/', "\\"), home.replace('\\', "/")]
    } else {
        vec![home.to_string()]
    };
    let mut out = text.to_string();
    for spelling in spellings {
        let case = if windows { "(?i)" } else { "" };
        let pattern = format!(r"{case}{}([\\/]|$|[^\w.~-])", regex::escape(&spelling));
        if let Ok(re) = regex::Regex::new(&pattern) {
            out = re.replace_all(&out, "~${1}").into_owned();
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

    fn tail(raw: &str) -> String {
        readable_tail(raw, "", &[])
    }

    #[test]
    fn colours_and_titles_are_removed() {
        let raw = "\x1b]0;Windows PowerShell\x07\x1b[31mError:\x1b[0m unknown flag --force\r\n";
        assert_eq!(tail(raw), "Error: unknown flag --force");
    }

    #[test]
    fn a_progress_bar_keeps_only_what_stayed_on_screen() {
        let raw = "Downloading\r\n  10%\r  55%\r 100%\r\nInstaller failed with exit code: 1603\r\n";
        assert_eq!(
            tail(raw),
            "Downloading\n 100%\nInstaller failed with exit code: 1603"
        );
    }

    #[test]
    fn a_posix_line_end_does_not_erase_the_line() {
        // ONLCR turns a program's own `\r\n` into `\r\r\n`, and a last line may end in `\r`.
        assert_eq!(
            tail("Error: checksum mismatch\r\r\nfatal: x\r"),
            "Error: checksum mismatch\nfatal: x"
        );
    }

    #[test]
    fn a_conpty_cursor_position_is_a_new_line_not_glue() {
        let raw = "\x1b[?25l\x1b[2J\x1b[HFound Orca [x]\x1b[K\x1b[2;1HNo applicable upgrade found.\x1b[K\x1b[?25h";
        assert_eq!(tail(raw), "Found Orca [x]\nNo applicable upgrade found.");
    }

    #[test]
    fn spaces_sent_as_cursor_moves_stay_spaces() {
        // ConPTY writes a run of spaces as erase + cursor-forward.
        let raw = "\x1b[HName\x1b[4X\x1b[4CId\x1b[2X\x1b[2CVersion\r\n";
        assert_eq!(tail(raw), "Name    Id  Version");
    }

    #[test]
    fn a_progress_bar_repainted_in_place_leaves_its_last_frame() {
        // ConPTY repaints the changed cells at an absolute position, frame after frame.
        let raw = "\x1b[H  Downloading\x1b[1;15H\u{2588}   25%\x1b[1;15H\u{2588}\u{2588}  50%\
                   \x1b[1;15H\u{2588}\u{2588}\u{2588} 100%\x1b[2;1HInstaller failed: 1603\x1b[3;1H";
        assert_eq!(
            tail(raw),
            "  Downloading \u{2588}\u{2588}\u{2588} 100%\nInstaller failed: 1603"
        );
    }

    #[test]
    fn a_line_longer_than_the_terminal_is_one_line() {
        let long = "x".repeat(250);
        assert_eq!(tail(&format!("{long}\r\nend\r\n")), format!("{long}\nend"));
    }

    #[test]
    fn only_the_last_lines_are_kept_and_blank_ones_do_not_count() {
        let raw: String = (1..=50).map(|n| format!("line {n}\r\n\r\n")).collect();
        let t = tail(&raw);
        assert_eq!(t.lines().count(), MAX_LINES);
        assert!(t.starts_with("line 31\n") && t.ends_with("line 50"), "{t}");
    }

    #[test]
    fn the_bytes_are_bounded_on_a_char_boundary_and_the_end_is_kept() {
        let raw: String = (0..100)
            .map(|_| format!("{}\r\n", "é".repeat(90)))
            .collect();
        let t = tail(&format!("{raw}the end\r\n"));
        assert!(t.len() <= MAX_BYTES, "{}", t.len());
        assert!(t.ends_with("the end"));
        assert!(t.starts_with('é'));
    }

    #[test]
    fn a_huge_output_renders_only_its_end() {
        let raw: String = (0..200_000).map(|n| format!("line {n}\r\n")).collect();
        assert!(tail(&raw).ends_with("line 199999"));
    }

    #[test]
    fn the_tail_stays_one_journal_line() {
        let line =
            serde_json::to_string(&serde_json::json!({ "tail": tail("a\r\nb\r\n") })).unwrap();
        assert!(!line.contains('\n'), "{line}");
    }

    #[test]
    fn the_home_folder_is_written_as_a_tilde_whatever_its_case_or_slashes() {
        let raw = "Log: c:\\users\\ALICE\\AppData\\Local\\Temp\\x.log\r\nsee C:/Users/alice/x\r\n";
        assert_eq!(
            readable_tail(raw, "C:\\Users\\alice", &[]),
            "Log: ~\\AppData\\Local\\Temp\\x.log\nsee ~/x"
        );
        assert_eq!(
            readable_tail("open /home/alice/.npmrc\n", "/home/alice", &[]),
            "open ~/.npmrc"
        );
    }

    #[test]
    fn any_profile_path_is_written_as_a_tilde() {
        // The 8.3 short form %TEMP% often carries, an MSYS HOME, and a home that is a prefix
        // of a longer name.
        let raw = "C:\\Users\\CHRIST~1\\AppData\\Local\\Temp\\i.log\r\n\
                   /c/Users/alice/x\r\n\
                   /Users/bob/Library\r\n";
        assert_eq!(
            readable_tail(raw, "/c/Users/alice", &[]),
            "~\\AppData\\Local\\Temp\\i.log\n~/x\n~/Library"
        );
        assert_eq!(
            readable_tail("/opt/alice2 and /opt/al/x\n", "/opt/al", &[]),
            "/opt/alice2 and ~/x"
        );
    }

    #[test]
    fn what_is_not_a_profile_is_left_alone() {
        // Shared folders and Linuxbrew's own prefix are nobody's home, and `users/` in a URL
        // or `home` inside a longer name is no profile at all.
        let raw = "/home/linuxbrew/.linuxbrew/bin/brew: failed\r\n\
                   C:\\Users\\Public\\Desktop\\x.lnk\r\n\
                   GET https://api.github.com/users/octocat/repos 403\r\n\
                   /srv/myhome/cfg and npm @users/pkg\r\n";
        assert_eq!(
            tail(raw),
            "/home/linuxbrew/.linuxbrew/bin/brew: failed\n\
             C:\\Users\\Public\\Desktop\\x.lnk\n\
             GET https://api.github.com/users/octocat/repos 403\n\
             /srv/myhome/cfg and npm @users/pkg"
        );
    }

    #[test]
    fn a_profile_name_with_a_space_is_masked_whole_and_the_text_after_it_kept() {
        let raw = "Log: C:\\Users\\John Smith\\AppData\\x.log\r\n\
                   /home/alice is missing\r\n";
        assert_eq!(tail(raw), "Log: ~\\AppData\\x.log\n~ is missing");
    }

    #[test]
    fn a_typed_answer_is_masked_however_the_terminal_renders_it() {
        // A trailing space is trimmed off the rendered line; the raw bytes still carry it.
        let raw = "Password: hunter2 \r\nSorry, try again.\r\n";
        assert_eq!(
            readable_tail(raw, "", &["hunter2 "]),
            "Password: ***\nSorry, try again."
        );
    }

    #[test]
    fn a_step_that_ends_in_the_alternate_screen_still_shows_its_main_screen() {
        let raw = "error: the real cause\r\n\x1b[?1049h\x1b[Hsome full-screen frame";
        assert_eq!(tail(raw), "error: the real cause");
    }

    #[test]
    fn credentials_in_a_url_are_masked_and_the_url_still_reads() {
        let raw = "fatal: unable to access 'https://bob:ghp_abc123@github.com/x/y.git/'\r\n\
                   fatal: unable to access 'https://ghp_AbC123@github.com/org/repo.git/'\r\n\
                   proxy http://DOMAIN%5Cbob:hunter2@proxy.corp:8080 refused\r\n\
                   plain https://github.com/x/y stays\r\n";
        assert_eq!(
            tail(raw),
            "fatal: unable to access 'https://***@github.com/x/y.git/'\n\
             fatal: unable to access 'https://***@github.com/org/repo.git/'\n\
             proxy http://***@proxy.corp:8080 refused\n\
             plain https://github.com/x/y stays"
        );
    }

    #[test]
    fn what_talos_typed_into_the_step_is_never_journalled() {
        // A prompt that echoes: the password Talos answered comes back as output.
        let raw = "Password: s3cr3t-pw\r\nSorry, try again.\r\n";
        assert_eq!(
            readable_tail(raw, "", &["s3cr3t-pw"]),
            "Password: ***\nSorry, try again."
        );
    }

    #[test]
    fn an_empty_output_has_an_empty_tail() {
        assert_eq!(tail(""), "");
        assert_eq!(readable_tail("\r\n\x1b[0m\r\n", "/home/a", &[]), "");
    }
}
