//! Stands in for 1Password's and Ente's command-line tools in the feature tests, so no real vault
//! or account is ever involved. It acts as whichever tool it has been copied and named as.
//!
//! Beside itself it appends each call to `calls.log`, and reads `mode.txt`: `signed-out` fails the
//! way a real tool does, and `slow:MS` waits first, like a fingerprint prompt.

fn main() {
    let exe = std::env::current_exe().unwrap_or_default();
    let name = exe.file_stem().map(|stem| stem.to_string_lossy().to_lowercase()).unwrap_or_default();
    let dir = exe.parent().map(std::path::Path::to_path_buf).unwrap_or_default();
    let args: Vec<String> = std::env::args().skip(1).collect();
    {
        use std::io::Write;
        if let Ok(mut log) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("calls.log")) {
            let _ = writeln!(log, "{name} {}", args.join(" "));
        }
    }
    let mode = std::fs::read_to_string(dir.join("mode.txt")).unwrap_or_default();
    let mode = mode.trim();
    if let Some(wait) = mode.strip_prefix("slow:").and_then(|ms| ms.parse().ok()) {
        std::thread::sleep(std::time::Duration::from_millis(wait));
    }
    if mode == "signed-out" {
        eprintln!("Error: no accounts; use account login --app auth");
        std::process::exit(1);
    }
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let output = match name.as_str() {
        "op" => match args.as_slice() {
            ["item", "list", ..] => Some(
                r#"[{"id":"qzitem1","title":"Qzgithub","category":"LOGIN","additional_information":"qzuser","vault":{"name":"Personal"},"urls":[{"primary":true,"href":"https://example.invalid"}]},
                    {"id":"qzitem2","title":"Qzrouter","category":"PASSWORD","vault":{"name":"Home"}},
                    {"id":"qzitem3","title":"","category":"LOGIN","vault":{"name":"Personal"}}]"#,
            ),
            ["item", "get", _, "--fields", "label=password", "--reveal"] => Some("fake-pass-Qz7"),
            ["item", "get", _, "--fields", "label=username"] => Some("fake-user-Qz7"),
            ["item", "get", _, "--otp"] => Some("654321"),
            _ => None,
        },
        "ente" => match args.as_slice() {
            ["auth", "code", "list", ..] => Some(r#"[{"id":"qzcode1","name":"Qzservice","issuer":"Qzservice","accountName":"qzuser","type":"totp"}]"#),
            ["auth", "code", "view", ..] => Some(
                r#"{"id":"qzcode1","name":"Qzservice","issuer":"Qzservice","accountName":"qzuser","type":"totp","otp":"123456","validUntil":"2099-01-01T00:00:00Z"}"#,
            ),
            _ => None,
        },
        _ => None,
    };
    match output {
        Some(text) => println!("{text}"),
        None => {
            eprintln!("unknown command");
            std::process::exit(2);
        }
    }
}
