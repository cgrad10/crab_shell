use std::env;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};

use rustyline::completion::{Completer, Pair};
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::history::DefaultHistory;
use rustyline::validate::Validator;
use rustyline::{Context, Editor, Helper};

const BUILTINS: &[&str] = &["echo", "exit", "type", "pwd", "cd"];

#[derive(PartialEq, Debug)]
enum Action {
    Continue,
    Exit,
}

#[derive(Debug, PartialEq, Clone, Copy)]
enum RedirectKind {
    Stdout,
    Stderr,
}

#[derive(Debug, PartialEq)]
struct Redirect {
    kind: RedirectKind,
    path: String,
    append: bool,
}

struct CommandResult {
    stdout: String,
    stderr: String,
    action: Action,
    redirect: Option<Redirect>,
}

impl CommandResult {
    fn cont() -> Self {
        Self {
            stdout: String::new(),
            stderr: String::new(),
            action: Action::Continue,
            redirect: None,
        }
    }
    fn out(s: String) -> Self {
        Self { stdout: s, ..Self::cont() }
    }
}

#[derive(Default, Clone)]
struct ShellEnv {
    path: String,
    home: String,
}

fn find_in_path(program: &str, path: &str) -> Option<PathBuf> {
    path.split(':')
        .map(|dir| Path::new(dir).join(program))
        .find(|p| {
            fs::metadata(p)
                .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
        })
}

fn builtin_type(arg: &str, shell: &ShellEnv) -> String {
    if BUILTINS.contains(&arg) {
        format!("{} is a shell builtin\n", arg)
    } else if let Some(filepath) = find_in_path(arg, &shell.path) {
        format!("{} is {}\n", arg, filepath.display())
    } else {
        format!("{}: not found\n", arg)
    }
}

fn run_external(args: &[String], shell: &ShellEnv) -> CommandResult {
    let Some((program, rest)) = args.split_first() else {
        return CommandResult::cont();
    };
    let Some(filepath) = find_in_path(program, &shell.path) else {
        return CommandResult::out(format!("{}: command not found\n", program));
    };
    match std::process::Command::new(&filepath)
        .arg0(program)
        .args(rest)
        .output()
    {
        Ok(o) => CommandResult {
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
            action: Action::Continue,
            redirect: None,
        },
        Err(e) => CommandResult::out(format!("{}: {}\n", program, e)),
    }
}

fn pwd() -> String {
    match env::current_dir() {
        Ok(o) => format!("{}\n", o.display()),
        Err(e) => format!("{}\n", e),
    }
}

fn cd(arg: &str, shell: &ShellEnv) -> String {
    let target: PathBuf = if arg == "~" {
        PathBuf::from(&shell.home)
    } else if let Some(rest) = arg.strip_prefix("~/") {
        Path::new(&shell.home).join(rest)
    } else {
        PathBuf::from(arg)
    };
    match env::set_current_dir(&target) {
        Ok(_) => String::new(),
        Err(_) => format!("cd: {}: No such file or directory\n", arg),
    }
}

fn parse(input: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut in_s_q = false;
    let mut in_d_q = false;
    let mut active = false;
    let mut escape = false;

    for c in input.chars() {
        if escape {
            if in_d_q && !matches!(c, '\\' | '$' | '"' | '\n') {
                cur.push('\\');
            }
            cur.push(c);
            active = true;
            escape = false;
            continue;
        }
        match c {
            '\\' if !in_s_q => { escape = true; }
            '"' if !in_s_q => { in_d_q = !in_d_q; active = true; }
            '\'' if !in_d_q => { in_s_q = !in_s_q; active = true; }
            c if c.is_whitespace() && !in_s_q && !in_d_q => {
                if active { args.push(std::mem::take(&mut cur)); active = false; }
            }
            c => { cur.push(c); active = true; }
        }
    }
    if active { args.push(cur); }
    args
}

fn split_redirect(tokens: Vec<String>) -> Result<(Vec<String>, Option<Redirect>), String> {
    const OPS: &[(&str, RedirectKind, bool)] = &[
        (">", RedirectKind::Stdout, false),
        ("1>", RedirectKind::Stdout, false),
        ("2>", RedirectKind::Stderr, false),
        (">>", RedirectKind::Stdout, true),
        ("1>>", RedirectKind::Stdout, true),
        ("2>>", RedirectKind::Stderr, true),
    ];

    for (i, tok) in tokens.iter().enumerate() {
        if let Some(&(_, kind, append)) = OPS.iter().find(|(op, _, _)| op == tok) {
            let path = match tokens.get(i + 1) {
                Some(p) => p.clone(),
                None => return Err(format!("syntax error near unexpected token `{}`\n", tok)),
            };
            let mut cmd = tokens;
            cmd.truncate(i);
            return Ok((cmd, Some(Redirect { kind, path, append })));
        }
    }
    Ok((tokens, None))
}

fn handle_command(command: &str, shell: &ShellEnv) -> CommandResult {
    let tokens = parse(command.trim());
    let (args, redirect) = match split_redirect(tokens) {
        Ok(v) => v,
        Err(msg) => {
            return CommandResult { stderr: msg, ..CommandResult::cont() };
        }
    };
    let Some((head, rest)) = args.split_first() else {
        return CommandResult { redirect, ..CommandResult::cont() };
    };
    let mut result = match head.as_str() {
        "exit" => CommandResult { action: Action::Exit, ..CommandResult::cont() },
        "echo" => CommandResult::out(format!("{}\n", rest.join(" "))),
        "type" => CommandResult::out(builtin_type(
            rest.first().map(String::as_str).unwrap_or(""),
            shell,
        )),
        "pwd" => CommandResult::out(pwd()),
        "cd" => CommandResult::out(cd(
            rest.first().map(String::as_str).unwrap_or(""),
            shell,
        )),
        _ => run_external(&args, shell),
    };
    result.redirect = redirect;
    result
}

fn write_stream(content: &str, redir: Option<&Redirect>, stream: RedirectKind) {
    match redir {
        Some(r) if r.kind == stream => {
            let res = if r.append {
                fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&r.path)
                    .and_then(|mut f| f.write_all(content.as_bytes()))
            } else {
                fs::write(&r.path, content)
            };
            if let Err(e) = res {
                eprintln!("{}: {}", r.path, e);
            }
        }
        _ => match stream {
            RedirectKind::Stdout => print!("{}", content),
            RedirectKind::Stderr => eprint!("{}", content),
        },
    }
}

fn complete_builtin(line: &str, pos: usize) -> (usize, Vec<&'static str>) {
    let prefix = line.get(..pos).unwrap_or("");
    if prefix.chars().any(char::is_whitespace) {
        return (pos, vec![]);
    }
    let matches: Vec<&'static str> = BUILTINS
        .iter()
        .copied()
        .filter(|b| b.starts_with(prefix))
        .collect();
    (0, matches)
}

fn complete_executables(line: &str, _pos: usize, shell: &ShellEnv) -> (usize, Vec<String>) {
    let mut matches: Vec<String> = Vec::new();
    for dir in shell.path.split(':') {
        let Ok(entries) = fs::read_dir(dir) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(line) {
                matches.push(name);
            }
        }
    }
    (0, matches)
}

struct ShellHelper{
    shellenv: ShellEnv
}

impl Completer for ShellHelper {
    type Candidate = Pair;
    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        let (_start2, names2) = complete_executables(line, pos, &self.shellenv);
        let (start, names) = complete_builtin(line, pos);
        let mut candidates: Vec<Pair> = names
            .into_iter()
            .map(|b| Pair {
                display: b.to_string(),
                replacement: format!("{} ", b),
            })
            .collect();
        let other_candidates: Vec<Pair> = names2
            .into_iter()
            .map(|b| Pair {
                display: b.clone(),
                replacement: format!("{} ", b),
            })
            .collect();
        candidates.extend(other_candidates);
        Ok((start, candidates))
    }
}

impl Hinter for ShellHelper {
    type Hint = String;
}
impl Highlighter for ShellHelper {}
impl Validator for ShellHelper {}
impl Helper for ShellHelper {}

fn main() {
    let shell = ShellEnv {
        path: env::var("PATH").unwrap_or_default(),
        home: env::var("HOME").unwrap_or_default(),
    };
    let mut editor: Editor<ShellHelper, DefaultHistory> =
        Editor::new().expect("failed to initialize line editor");
    let shell_helper = ShellHelper{
        shellenv: shell.clone()
    };
    editor.set_helper(Some(shell_helper));

    loop {
        let command = match editor.readline("$ ") {
            Ok(line) => line,
            Err(ReadlineError::Eof) => break,
            Err(ReadlineError::Interrupted) => continue,
            Err(e) => {
                eprintln!("readline error: {}", e);
                break;
            }
        };
        let result = handle_command(&command, &shell);
        write_stream(&result.stdout, result.redirect.as_ref(), RedirectKind::Stdout);
        write_stream(&result.stderr, result.redirect.as_ref(), RedirectKind::Stderr);
        io::stdout().flush().ok();
        if result.action == Action::Exit {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::TempDir;

    static CWD_LOCK: Mutex<()> = Mutex::new(());

    fn lock_cwd() -> std::sync::MutexGuard<'static, ()> {
        CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn run(input: &str, shell: &ShellEnv) -> (String, Action) {
        let r = handle_command(input, shell);
        (r.stdout, r.action)
    }

    fn write_executable(dir: &Path, name: &str, script: &str) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, script).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    fn write_non_executable(dir: &Path, name: &str) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, "").unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o644)).unwrap();
        p
    }

    #[test]
    fn test_echo_returns_continue_and_expected_characters_with_newline() {
        let (output, action) = run("echo pineapple blueberry orange", &ShellEnv::default());
        assert_eq!(output, "pineapple blueberry orange\n");
        assert_eq!(action, Action::Continue);
    }

    #[test]
    fn test_echo_with_no_args_returns_just_newline() {
        let (output, action) = run("echo", &ShellEnv::default());
        assert_eq!(output, "\n");
        assert_eq!(action, Action::Continue);
    }

    #[test]
    fn test_invalid_command_returns_continue_and_command_not_found_message() {
        let (output, action) = run("invalid_apple_command", &ShellEnv::default());
        assert_eq!(output, "invalid_apple_command: command not found\n");
        assert_eq!(action, Action::Continue);
    }

    #[test]
    fn test_exit_returns_exit() {
        let (_output, action) = run("exit", &ShellEnv::default());
        assert_eq!(action, Action::Exit);
    }

    #[test]
    fn test_handle_command_trims_whitespace_and_newline() {
        let (output, action) = run("   echo hi  \n", &ShellEnv::default());
        assert_eq!(output, "hi\n");
        assert_eq!(action, Action::Continue);
    }

    #[test]
    fn test_type_reports_each_builtin_as_shell_builtin() {
        for builtin in ["echo", "exit", "type"] {
            let (output, action) = run(&format!("type {}", builtin), &ShellEnv::default());
            assert_eq!(output, format!("{} is a shell builtin\n", builtin));
            assert_eq!(action, Action::Continue);
        }
    }

    #[test]
    fn test_type_reports_not_found_for_unknown_command() {
        let (output, action) = run("type definitely_not_a_real_cmd", &ShellEnv::default());
        assert_eq!(output, "definitely_not_a_real_cmd: not found\n");
        assert_eq!(action, Action::Continue);
    }

    #[test]
    fn test_type_finds_executable_in_path() {
        let tmp = TempDir::new().unwrap();
        let exe = write_executable(tmp.path(), "my_tool", "#!/bin/sh\n");
        let path = tmp.path().to_string_lossy();
        let (output, _) = run("type my_tool", &ShellEnv { path: path.into(), ..Default::default() });
        assert_eq!(output, format!("my_tool is {}\n", exe.display()));
    }

    #[test]
    fn test_type_skips_non_executable_file() {
        let tmp = TempDir::new().unwrap();
        write_non_executable(tmp.path(), "not_runnable");
        let path = tmp.path().to_string_lossy();
        let (output, _) = run("type not_runnable", &ShellEnv { path: path.into(), ..Default::default() });
        assert_eq!(output, "not_runnable: not found\n");
    }

    #[test]
    fn test_find_in_path_returns_none_for_missing_program() {
        assert!(find_in_path("totally_made_up_program", "/nonexistent_dir_xyz").is_none());
    }

    #[test]
    fn test_find_in_path_handles_empty_path() {
        assert!(find_in_path("anything", "").is_none());
    }

    #[test]
    fn test_find_in_path_searches_multiple_directories() {
        let tmp = TempDir::new().unwrap();
        let exe = write_executable(tmp.path(), "found_me", "#!/bin/sh\n");
        let path = format!("/nonexistent_a:{}:/nonexistent_b", tmp.path().display());
        assert_eq!(find_in_path("found_me", &path).unwrap(), exe);
    }

    #[test]
    fn test_run_external_executes_program_and_returns_stdout() {
        let tmp = TempDir::new().unwrap();
        write_executable(tmp.path(), "say_hi", "#!/bin/sh\necho hello world\n");
        let path = tmp.path().to_string_lossy();
        let (output, action) = run("say_hi", &ShellEnv { path: path.into(), ..Default::default() });
        assert_eq!(output, "hello world\n");
        assert_eq!(action, Action::Continue);
    }

    #[test]
    fn test_run_external_passes_arguments() {
        let tmp = TempDir::new().unwrap();
        write_executable(tmp.path(), "echo_args", "#!/bin/sh\necho \"$1-$2\"\n");
        let path = tmp.path().to_string_lossy();
        let (output, _) = run("echo_args foo bar", &ShellEnv { path: path.into(), ..Default::default() });
        assert_eq!(output, "foo-bar\n");
    }

    #[test]
    fn test_run_external_splits_stdout_and_stderr() {
        let tmp = TempDir::new().unwrap();
        write_executable(
            tmp.path(),
            "noisy",
            "#!/bin/sh\necho to-out\necho to-err 1>&2\n",
        );
        let path = tmp.path().to_string_lossy();
        let r = handle_command("noisy", &ShellEnv { path: path.into(), ..Default::default() });
        assert_eq!(r.stdout, "to-out\n");
        assert_eq!(r.stderr, "to-err\n");
    }

    #[test]
    fn test_pwd_returns_current_directory_with_newline() {
        let _guard = lock_cwd();
        let expected = format!("{}\n", env::current_dir().unwrap().display());
        let (output, action) = run("pwd", &ShellEnv::default());
        assert_eq!(output, expected);
        assert_eq!(action, Action::Continue);
    }

    #[test]
    fn test_cd_changes_to_absolute_path_and_pwd_reflects_it() {
        let _guard = lock_cwd();
        let original = env::current_dir().unwrap();
        let tmp = TempDir::new().unwrap();
        let target = fs::canonicalize(tmp.path()).unwrap();

        let (output, action) = run(&format!("cd {}", target.display()), &ShellEnv::default());
        assert_eq!(output, "");
        assert_eq!(action, Action::Continue);

        let (pwd_out, _) = run("pwd", &ShellEnv::default());
        assert_eq!(pwd_out, format!("{}\n", target.display()));

        env::set_current_dir(original).unwrap();
    }

    #[test]
    fn test_cd_invalid_path_returns_error_message() {
        let _guard = lock_cwd();
        let (output, action) = run("cd /definitely_not_a_real_dir_apple_xyz", &ShellEnv::default());
        assert_eq!(
            output,
            "cd: /definitely_not_a_real_dir_apple_xyz: No such file or directory\n"
        );
        assert_eq!(action, Action::Continue);
    }

    #[test]
    fn test_cd_tilde_changes_to_home_directory() {
        let _guard = lock_cwd();
        let original = env::current_dir().unwrap();
        let tmp = TempDir::new().unwrap();
        let home = fs::canonicalize(tmp.path()).unwrap();
        let home_str = home.to_string_lossy();

        let (output, action) = run("cd ~", &ShellEnv { home: home_str.into(), ..Default::default() });
        assert_eq!(output, "");
        assert_eq!(action, Action::Continue);

        let (pwd_out, _) = run("pwd", &ShellEnv::default());
        assert_eq!(pwd_out, format!("{}\n", home.display()));

        env::set_current_dir(original).unwrap();
    }

    #[test]
    fn test_cd_tilde_slash_subpath_resolves_relative_to_home() {
        let _guard = lock_cwd();
        let original = env::current_dir().unwrap();
        let tmp = TempDir::new().unwrap();
        let home = fs::canonicalize(tmp.path()).unwrap();
        fs::create_dir(home.join("sub")).unwrap();
        let home_str = home.to_string_lossy();

        let (output, action) =
            run("cd ~/sub", &ShellEnv { home: home_str.into(), ..Default::default() });
        assert_eq!(output, "");
        assert_eq!(action, Action::Continue);

        let (pwd_out, _) = run("pwd", &ShellEnv::default());
        assert_eq!(pwd_out, format!("{}\n", home.join("sub").display()));

        env::set_current_dir(original).unwrap();
    }

    #[test]
    fn test_cd_tilde_with_invalid_home_returns_error() {
        let _guard = lock_cwd();
        let (output, action) = run(
            "cd ~",
            &ShellEnv { home: "/definitely_not_a_real_home_xyz_apple".into(), ..Default::default() },
        );
        assert_eq!(output, "cd: ~: No such file or directory\n");
        assert_eq!(action, Action::Continue);
    }

    #[test]
    fn test_split_redirect_basic_stdout() {
        let tokens = vec!["echo".into(), "hi".into(), ">".into(), "out.txt".into()];
        let (cmd, redir) = split_redirect(tokens).unwrap();
        assert_eq!(cmd, vec!["echo".to_string(), "hi".into()]);
        let r = redir.unwrap();
        assert_eq!(r.kind, RedirectKind::Stdout);
        assert_eq!(r.path, "out.txt");
        assert!(!r.append);
    }

    #[test]
    fn test_split_redirect_1_prefix_is_stdout() {
        let tokens = vec!["echo".into(), "1>".into(), "out.txt".into()];
        let r = split_redirect(tokens).unwrap().1.unwrap();
        assert_eq!(r.kind, RedirectKind::Stdout);
    }

    #[test]
    fn test_split_redirect_stderr() {
        let tokens = vec!["ls".into(), "2>".into(), "err.txt".into()];
        let r = split_redirect(tokens).unwrap().1.unwrap();
        assert_eq!(r.kind, RedirectKind::Stderr);
        assert!(!r.append);
    }

    #[test]
    fn test_split_redirect_append_variants() {
        for (op, kind) in [
            (">>", RedirectKind::Stdout),
            ("1>>", RedirectKind::Stdout),
            ("2>>", RedirectKind::Stderr),
        ] {
            let tokens = vec!["x".into(), op.into(), "p".into()];
            let r = split_redirect(tokens).unwrap().1.unwrap();
            assert!(r.append, "{} should be append", op);
            assert_eq!(r.kind, kind);
        }
    }

    #[test]
    fn test_split_redirect_no_operator() {
        let tokens = vec!["echo".into(), "hi".into()];
        let (_, redir) = split_redirect(tokens).unwrap();
        assert!(redir.is_none());
    }

    #[test]
    fn test_split_redirect_missing_path_is_error() {
        let tokens = vec!["echo".into(), ">".into()];
        assert!(split_redirect(tokens).is_err());
    }

    #[test]
    fn test_redirect_inside_double_quotes_is_literal() {
        let r = handle_command("echo \"1 > 2\"", &ShellEnv::default());
        assert_eq!(r.stdout, "1 > 2\n");
        assert!(r.redirect.is_none());
    }

    #[test]
    fn test_handle_command_attaches_redirect_to_result() {
        let r = handle_command("echo hi > /tmp/out", &ShellEnv::default());
        assert_eq!(r.stdout, "hi\n");
        let redir = r.redirect.unwrap();
        assert_eq!(redir.path, "/tmp/out");
        assert_eq!(redir.kind, RedirectKind::Stdout);
    }

    #[test]
    fn test_write_stream_writes_stdout_to_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("out.txt");
        let redir = Redirect {
            kind: RedirectKind::Stdout,
            path: path.to_string_lossy().into(),
            append: false,
        };
        write_stream("hello\n", Some(&redir), RedirectKind::Stdout);
        assert_eq!(fs::read_to_string(&path).unwrap(), "hello\n");
    }

    #[test]
    fn test_write_stream_append_appends_to_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("out.txt");
        let redir = Redirect {
            kind: RedirectKind::Stdout,
            path: path.to_string_lossy().into(),
            append: true,
        };
        write_stream("one\n", Some(&redir), RedirectKind::Stdout);
        write_stream("two\n", Some(&redir), RedirectKind::Stderr); // mismatched stream, no-op for file
        write_stream("three\n", Some(&redir), RedirectKind::Stdout);
        assert_eq!(fs::read_to_string(&path).unwrap(), "one\nthree\n");
    }

    #[test]
    fn test_complete_builtin_unique_prefix_returns_single_match() {
        assert_eq!(complete_builtin("ech", 3), (0, vec!["echo"]));
        assert_eq!(complete_builtin("exi", 3), (0, vec!["exit"]));
    }

    #[test]
    fn test_complete_builtin_no_match_returns_empty() {
        let (_, names) = complete_builtin("xyz", 3);
        assert!(names.is_empty());
    }

    #[test]
    fn test_complete_builtin_after_space_returns_empty() {
        let (start, names) = complete_builtin("echo ", 5);
        assert_eq!(start, 5);
        assert!(names.is_empty());
    }

    #[test]
    fn test_complete_builtin_shared_prefix_returns_all_matches() {
        let (start, names) = complete_builtin("e", 1);
        assert_eq!(start, 0);
        assert!(names.contains(&"echo"));
        assert!(names.contains(&"exit"));
    }

    #[test]
    fn test_redirect_external_stderr_separately() {
        let tmp = TempDir::new().unwrap();
        write_executable(
            tmp.path(),
            "noisy",
            "#!/bin/sh\necho to-out\necho to-err 1>&2\n",
        );
        let path = tmp.path().to_string_lossy();
        let err_path = tmp.path().join("err.txt");
        let r = handle_command(
            &format!("noisy 2> {}", err_path.display()),
            &ShellEnv { path: path.into(), ..Default::default() },
        );
        assert_eq!(r.stdout, "to-out\n");
        let redir = r.redirect.as_ref().unwrap();
        assert_eq!(redir.kind, RedirectKind::Stderr);
        write_stream(&r.stderr, Some(redir), RedirectKind::Stderr);
        assert_eq!(fs::read_to_string(&err_path).unwrap(), "to-err\n");
    }
}
