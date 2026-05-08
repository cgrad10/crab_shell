use std::env;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;

const BUILTINS: &[&str] = &["echo", "exit", "type", "pwd", "cd"];

#[derive(PartialEq, Debug)]
enum Action {
    Continue,
    Exit,
}

#[derive(Default)]
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

fn run_external(command: &str, shell: &ShellEnv) -> String {
    let mut parts = command.split_whitespace();
    let program = parts.next().unwrap_or("");
    let args: Vec<&str> = parts.collect();
    let Some(filepath) = find_in_path(program, &shell.path) else {
        return format!("{}: command not found\n", program);
    };
    match std::process::Command::new(&filepath)
        .arg0(program)
        .args(&args)
        .output()
    {
        Ok(o) => String::from_utf8_lossy(&o.stdout).into_owned(),
        Err(e) => format!("{}: {}\n", program, e),
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

fn is_single_quoted(s: &str) -> bool {
    s.starts_with('\'') && s.ends_with('\'')
}

fn handle_single_quotes(s: &str) -> String{
    // 'hello    world' -> hello   world
    // hello    world -> hello world
    if is_single_quoted(s){
        s[1..s.len() - 1].to_string()
    } else {
        s.split_whitespace().collect::<Vec<_>>().join(" ").to_string()
    }
}

fn parse(input: &str) -> String {
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut in_q = false;
    let mut active = false;  // is there a token in progress?

    for c in input.chars() {
        match c {
            '\'' => { in_q = !in_q; active = true; }
            c if c.is_whitespace() && !in_q => {
                if active { args.push(std::mem::take(&mut cur)); active = false; }
            }
            c => { cur.push(c); active = true; }
        }
    }
    if active { args.push(cur); }
    args.join(" ")
}

fn handle_command(command: &str, shell: &ShellEnv) -> (String, Action) {
    let command = command.trim();
    let (head, rest) = command.split_once(' ').unwrap_or((command, ""));
    match head {
        "exit" => (String::new(), Action::Exit),
        "echo" => (format!("{}\n", parse(rest)), Action::Continue),
        "type" => (builtin_type(rest, shell), Action::Continue),
        "pwd" => (pwd(), Action::Continue),
        "cd" => (cd(rest, shell), Action::Continue),
        _ => (run_external(command, shell), Action::Continue),
    }
}

fn read_line() -> String {
    print!("$ ");
    io::stdout().flush().unwrap();
    let mut command = String::new();
    io::stdin().read_line(&mut command).unwrap();
    command
}

fn main() {
    let shell = ShellEnv {
        path: env::var("PATH").unwrap_or_default(),
        home: env::var("HOME").unwrap_or_default(),
    };
    loop {
        let command = read_line();
        let (out, action) = handle_command(&command, &shell);
        print!("{}", out);
        if action == Action::Exit {
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
        let (output, action) = handle_command("echo pineapple blueberry orange", &ShellEnv::default());
        assert_eq!(output, "pineapple blueberry orange\n");
        assert_eq!(action, Action::Continue);
    }

    #[test]
    fn test_echo_with_no_args_returns_just_newline() {
        let (output, action) = handle_command("echo", &ShellEnv::default());
        assert_eq!(output, "\n");
        assert_eq!(action, Action::Continue);
    }

    #[test]
    fn test_invalid_command_returns_continue_and_command_not_found_message() {
        let (output, action) = handle_command("invalid_apple_command", &ShellEnv::default());
        assert_eq!(output, "invalid_apple_command: command not found\n");
        assert_eq!(action, Action::Continue);
    }

    #[test]
    fn test_exit_returns_exit() {
        let (_output, action) = handle_command("exit", &ShellEnv::default());
        assert_eq!(action, Action::Exit);
    }

    #[test]
    fn test_handle_command_trims_whitespace_and_newline() {
        let (output, action) = handle_command("   echo hi  \n", &ShellEnv::default());
        assert_eq!(output, "hi\n");
        assert_eq!(action, Action::Continue);
    }

    #[test]
    fn test_type_reports_each_builtin_as_shell_builtin() {
        for builtin in ["echo", "exit", "type"] {
            let (output, action) = handle_command(&format!("type {}", builtin), &ShellEnv::default());
            assert_eq!(output, format!("{} is a shell builtin\n", builtin));
            assert_eq!(action, Action::Continue);
        }
    }

    #[test]
    fn test_type_reports_not_found_for_unknown_command() {
        let (output, action) = handle_command("type definitely_not_a_real_cmd", &ShellEnv::default());
        assert_eq!(output, "definitely_not_a_real_cmd: not found\n");
        assert_eq!(action, Action::Continue);
    }

    #[test]
    fn test_type_finds_executable_in_path() {
        let tmp = TempDir::new().unwrap();
        let exe = write_executable(tmp.path(), "my_tool", "#!/bin/sh\n");
        let path = tmp.path().to_string_lossy();
        let (output, _) = handle_command("type my_tool", &ShellEnv { path: path.into(), ..Default::default() });
        assert_eq!(output, format!("my_tool is {}\n", exe.display()));
    }

    #[test]
    fn test_type_skips_non_executable_file() {
        let tmp = TempDir::new().unwrap();
        write_non_executable(tmp.path(), "not_runnable");
        let path = tmp.path().to_string_lossy();
        let (output, _) = handle_command("type not_runnable", &ShellEnv { path: path.into(), ..Default::default() });
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
        let (output, action) = handle_command("say_hi", &ShellEnv { path: path.into(), ..Default::default() });
        assert_eq!(output, "hello world\n");
        assert_eq!(action, Action::Continue);
    }

    #[test]
    fn test_run_external_passes_arguments() {
        let tmp = TempDir::new().unwrap();
        write_executable(tmp.path(), "echo_args", "#!/bin/sh\necho \"$1-$2\"\n");
        let path = tmp.path().to_string_lossy();
        let (output, _) = handle_command("echo_args foo bar", &ShellEnv { path: path.into(), ..Default::default() });
        assert_eq!(output, "foo-bar\n");
    }

    #[test]
    fn test_pwd_returns_current_directory_with_newline() {
        let _guard = lock_cwd();
        let expected = format!("{}\n", env::current_dir().unwrap().display());
        let (output, action) = handle_command("pwd", &ShellEnv::default());
        assert_eq!(output, expected);
        assert_eq!(action, Action::Continue);
    }

    #[test]
    fn test_cd_changes_to_absolute_path_and_pwd_reflects_it() {
        let _guard = lock_cwd();
        let original = env::current_dir().unwrap();
        let tmp = TempDir::new().unwrap();
        let target = fs::canonicalize(tmp.path()).unwrap();

        let (output, action) =
            handle_command(&format!("cd {}", target.display()), &ShellEnv::default());
        assert_eq!(output, "");
        assert_eq!(action, Action::Continue);

        let (pwd_out, _) = handle_command("pwd", &ShellEnv::default());
        assert_eq!(pwd_out, format!("{}\n", target.display()));

        env::set_current_dir(original).unwrap();
    }

    #[test]
    fn test_cd_invalid_path_returns_error_message() {
        let _guard = lock_cwd();
        let (output, action) =
            handle_command("cd /definitely_not_a_real_dir_apple_xyz", &ShellEnv::default());
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

        let (output, action) = handle_command("cd ~", &ShellEnv { home: home_str.into(), ..Default::default() });
        assert_eq!(output, "");
        assert_eq!(action, Action::Continue);

        let (pwd_out, _) = handle_command("pwd", &ShellEnv::default());
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
            handle_command("cd ~/sub", &ShellEnv { home: home_str.into(), ..Default::default() });
        assert_eq!(output, "");
        assert_eq!(action, Action::Continue);

        let (pwd_out, _) = handle_command("pwd", &ShellEnv::default());
        assert_eq!(pwd_out, format!("{}\n", home.join("sub").display()));

        env::set_current_dir(original).unwrap();
    }

    #[test]
    fn test_cd_tilde_with_invalid_home_returns_error() {
        let _guard = lock_cwd();
        let (output, action) =
            handle_command("cd ~", &ShellEnv { home: "/definitely_not_a_real_home_xyz_apple".into(), ..Default::default() });
        assert_eq!(output, "cd: ~: No such file or directory\n");
        assert_eq!(action, Action::Continue);
    }
}
