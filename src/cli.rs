// Command line, modelled on FileZilla's:
//   skiff [URL] [-c|--site <site>] [-a|--local <dir>]
// URL: protocol://user:password@host:port/path

pub const HELP: &str = "\
Skiff — FTP / FTPS / SFTP клиент

Использование:
  skiff [адрес] [параметры]

Адрес:
  протокол://логин:пароль@сервер:порт/папка
  Протоколы: ftp, ftpes (явный TLS), ftps (неявный TLS), sftp.
  Логин, пароль, порт и папка необязательны. Если в пароле есть
  символы @ : / % или пробел, закодируйте их: @ → %40, : → %3A,
  / → %2F, % → %25, пробел → %20.

Параметры:
  -c, --site <имя>    подключиться к сайту из менеджера сайтов
                      (\"Имя\", \"Группа/Имя\" или \"0/Группа/Имя\" как в FileZilla)
  -a, --local <папка> открыть эту локальную папку
  -h, --help          показать эту справку
  -v, --version       показать версию

Примеры:
  skiff sftp://root:пароль@192.168.1.10
  skiff ftpes://user@ftp.example.com:21/www
  skiff --site \"Продакшн/Веб-сервер\" --local D:\\Сайт";

#[derive(Debug, Default, PartialEq)]
pub struct Launch {
    pub url: Option<String>,
    pub site: Option<String>,
    pub local: Option<String>,
}

pub enum Parsed {
    Run(Launch),
    Help,
    Version,
    Error(String),
}

pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Parsed {
    let mut out = Launch::default();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let (key, inline) = match a.split_once('=') {
            Some((k, v)) if k.starts_with("--") => (k.to_string(), Some(v.to_string())),
            _ => (a.clone(), None),
        };
        let mut value = |name: &str| -> Result<String, String> {
            inline
                .clone()
                .or_else(|| it.next())
                .ok_or_else(|| format!("для {name} нужно значение"))
        };
        let res = match key.as_str() {
            "-h" | "--help" | "/?" => return Parsed::Help,
            "-v" | "--version" => return Parsed::Version,
            "-c" | "--site" => value(&key).map(|v| out.site = Some(v)),
            "-a" | "--local" => value(&key).map(|v| out.local = Some(v)),
            // FileZilla flags that make no sense here are accepted and ignored
            "-l" | "--logontype" => value(&key).map(|_| ()),
            "-s" | "--site-manager" | "--verbose" => Ok(()),
            s if s.starts_with('-') && s.len() > 1 => Err(format!("неизвестный параметр «{s}»")),
            _ if out.url.is_none() => {
                out.url = Some(a);
                Ok(())
            }
            _ => Err(format!("лишний аргумент «{a}»")),
        };
        if let Err(e) = res {
            return Parsed::Error(e);
        }
    }
    Parsed::Run(out)
}

/// Shows text to the user: stdout when there is a console, a message box
/// otherwise (a Windows GUI build has no console to print to).
pub fn show(text: &str, error: bool) {
    println!("{text}");
    if cfg!(all(target_os = "windows", not(debug_assertions))) {
        let _ = rfd::MessageDialog::new()
            .set_title("Skiff")
            .set_description(text)
            .set_level(if error { rfd::MessageLevel::Error } else { rfd::MessageLevel::Info })
            .show();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(a: &[&str]) -> Launch {
        match parse(a.iter().map(|s| s.to_string())) {
            Parsed::Run(l) => l,
            _ => panic!("not run"),
        }
    }

    #[test]
    fn parses_args() {
        assert_eq!(run(&["sftp://root:pw@1.2.3.4"]).url.as_deref(), Some("sftp://root:pw@1.2.3.4"));
        let l = run(&["--site=0/Prod/Web", "-a", "/tmp"]);
        assert_eq!(l.site.as_deref(), Some("0/Prod/Web"));
        assert_eq!(l.local.as_deref(), Some("/tmp"));
        assert!(matches!(parse(["--bogus".to_string()]), Parsed::Error(_)));
        assert!(matches!(parse(["-c".to_string()]), Parsed::Error(_)));
        assert!(matches!(parse(["--help".to_string()]), Parsed::Help));
    }
}
