#[allow(unused_imports)]
use std::io::{self, Write};
use std::path::Path;
use std::fs;
use std::os::unix::fs::PermissionsExt;

#[derive(PartialEq, Debug)]
enum Action {
    Continue,
    Exit,
}

fn handle_command(command: &str, path: &str) -> (String, Action) {
    let command = command.trim();
    if command.starts_with("type ") {
        let substring = &command[5..];
        if substring == "echo" || substring == "exit" || substring == "type" {
            (format!("{} is a shell builtin\n", substring), Action::Continue)
        } else {
            for dir in path.split(":") {
                let filepath = Path::new(dir).join(substring);
                if let Ok(metadata) = fs::metadata(&filepath) {
                    if metadata.is_file() && metadata.permissions().mode() & 0o111 != 0 {
                        return (format!("{} is {}\n", substring, filepath.display()), Action::Continue);
                    }
                }
            }
            (format!("{}: not found\n", substring), Action::Continue)
        }
    } else if command.starts_with("echo ") {
        (format!("{}\n", &command[5..]), Action::Continue)
    } else if command == "exit" {
        (String::new(), Action::Exit)
    } else {
        (format!("{}: command not found\n", command), Action::Continue)
    }
}

fn main() {

    let path = std::env::var("PATH").unwrap_or("".to_string());
    loop{
        print!("$ ");
        io::stdout().flush().unwrap();
        let mut command = String::new();
        io::stdin().read_line(&mut command).unwrap();
        let (out, action) = handle_command(&command, &path);
        print!("{}", out);
        if let Action::Exit = action {
            break;
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_echo_returns_continue_and_expected_characters_with_newline() {
        //arrange
        let input: &str = "echo pineapple blueberry orange";
        //act
        let (output_result, action_result) = handle_command(input, "");
        //assert
        assert_eq!(output_result, "pineapple blueberry orange\n");
        assert_eq!(action_result, Action::Continue);
    }

    #[test]
    fn test_invalid_command_returns_continue_and_command_not_found_message() {
        //arrange
        let input: &str = "invalid_apple_command";
        //act
        let (output_result, action_result) = handle_command(input, "");
        //assert
        assert_eq!(output_result, "invalid_apple_command: command not found\n");
        assert_eq!(action_result, Action::Continue);
    }

    #[test]
    fn test_exit_returns_exit() {
        //arrange
        let input: &str = "exit";
        //act
        let (_output_result, action_result) = handle_command(input, "");
        //assert
        assert_eq!(action_result, Action::Exit);
    }
}
