use clap::Parser;
use reqwest::blocking::Client;
use serde_json::{Value, json};
use std::io::{self, Write};
use std::time::Duration;

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "http://127.0.0.1:7878")]
    url: String,
}
fn input(prompt: &str) -> io::Result<String> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut line = String::new();
    if io::stdin().read_line(&mut line)? == 0 {
        return Err(io::ErrorKind::UnexpectedEof.into());
    }
    Ok(line.trim_end_matches(['\r', '\n']).to_owned())
}

fn text_input() -> io::Result<String> {
    println!("Text: enter . alone to finish; start a dot-leading line with an extra dot.");
    println!(
        "No implicit trailing newline; add a blank line to include one. . immediately means empty text."
    );

    let mut lines = Vec::new(); //save multi-line text

    loop {
        let line = input("| ")?;
        let is_terminator = line == "."; //ture when entire line is '.'
        lines.push(line);
        if is_terminator {
            break;
        }
    }

    Ok(decode_text_lines(lines))
}

fn decode_text_lines(lines: impl IntoIterator<Item = String>) -> String {
    let mut text_lines = Vec::new();

    for line in lines {
        if line == "." {
            break;
        }

        // A doubled leading dot escapes one dot
        let line = if let Some(rest) = line.strip_prefix("..") {
            format!(".{rest}") // return one '.' before rest
        } else {
            line
        };
        text_lines.push(line);
    }

    text_lines.join("\n")
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let client = Client::builder()
        .timeout(Duration::from_secs(12))
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let mut token = String::new();
    loop {
        let command = match input(
            "ping / register / login / logout / list / echo / delete-user / put / get / delete / q > ",
        ) {
            Ok(command) => command,
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(error) => return Err(error.into()),
        };
        let mut body = Value::Null;
        let mut dynamic_path: Option<String> = None; // add dynamic path variables
        let (method, path) = match command.as_str() {
            "q" => break,
            "ping" => ("GET", "/ping"),
            "list" => ("GET", "/texts"),
            "logout" => ("DELETE", "/sessions/current"),
            "register" | "login" => {
                body = json!({"username": input("username: ")?, "password": rpassword::prompt_password("password: ")?});
                (
                    "POST",
                    if command == "register" {
                        "/users"
                    } else {
                        "/sessions"
                    },
                )
            }

            "echo" => {
                let text = text_input()?;
                body = json!({"text": text});
                ("POST", "/echo")
            }

            "put" => {
                let name = input("name: ")?;
                let text = text_input()?;

                dynamic_path = Some(format!("/texts/{name}"));
                body = json!({"text": text});

                ("PUT", "")
            }

            "get" => {
                let name = input("name: ")?;
                dynamic_path = Some(format!("/texts/{name}"));

                ("GET", "")
            }

            "delete" => {
                let name = input("name: ")?;
                dynamic_path = Some(format!("/texts/{name}"));

                ("DELETE", "")
            }

            "delete-user" => ("DELETE", "/users/me"),

            _ => {
                println!("Unknown command.");
                continue;
            }
        };

        let request_path = dynamic_path.as_deref().unwrap_or(path); // select the final path
        let result = rm_client_sync::exchange(
            &client,
            &args.url,
            method.parse().unwrap(),
            request_path,
            &token,
            if body.is_null() { None } else { Some(&body) },
        );
        match result {
            Ok((status, value)) => {
                if matches!(command.as_str(), "echo" | "get" | "put"){
                    println!("HTTP {status}");
                    if let Some(text) = value.get("data").and_then(Value::as_str) {
                        println!("{text}");
                    } else {
                        println!("{value}");
                    }
                } else {
                    println!("{status} {value}");
                }
                if command == "login"
                    && status == 200
                    && let Some(next) = value["data"]["token"].as_str()
                {
                    token = next.into();
                }
                if status == 401 {
                    println!("Please log in again.");
                }
                if status == 401
                    || ((command == "logout" || command == "delete-user") && status == 200)
                {
                    // clear token when 401 or logout/delete
                    token.clear();
                }
            }
            Err(error) => eprintln!("Request failed: {error}"),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::decode_text_lines;

    fn lines(items: &[&str]) -> Vec<String> {
        items.iter().map(|line| (*line).to_owned()).collect()
    }

    #[test]
    fn echo_text_input_matches_reference_dot_rules_and_newlines() {
        let text = decode_text_lines(lines(&["..hello", ".hello", "..", "", "."]));
        assert_eq!(text, ".hello\n.hello\n.\n");
    }

    #[test]
    fn a_dot_terminator_immediately_means_empty_text() {
        assert_eq!(decode_text_lines(lines(&["."])), "");
    }

    #[test]
    fn text_without_a_blank_final_line_has_no_trailing_newline() {
        assert_eq!(decode_text_lines(lines(&["hello", "."])), "hello");
    }
}
