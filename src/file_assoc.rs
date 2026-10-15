//! Ассоциация с `.md` / `.markdown` для текущего пользователя (HKCU, без прав администратора).
//!
//! Windows 10/11 не даёт программе самой назначить себя приложением по умолчанию: выбор
//! (`UserChoice`) защищён хэшем и делается только пользователем. Поэтому мы регистрируемся —
//! ProgID с иконкой и командой, «Открыть с помощью», «Приложения по умолчанию» в
//! Параметрах, — а затем открываем Параметры, где пользователь выбирает Markdowntown.
//!
//! Exe portable и может переехать: [`status`] сообщает, если регистрация указывает на
//! другую копию.

use std::io;
use std::path::{Path, PathBuf};

use winreg::RegKey;
use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};

pub const EXTENSIONS: [&str; 2] = [".md", ".markdown"];
const PROG_ID: &str = "Markdowntown.md";
const APP_NAME: &str = "Markdowntown";
const EXE_NAME: &str = "markdowntown.exe";

/// Где в HKCU лежат ключи. Тесты используют отдельную ветку, а не настоящие ассоциации.
pub struct Location {
    /// Аналог `Software\Classes`.
    classes: String,
    /// Аналог `Software` — для `Capabilities` и `RegisteredApplications`.
    software: String,
}

impl Location {
    pub fn user() -> Self {
        Self {
            classes: r"Software\Classes".into(),
            software: "Software".into(),
        }
    }

    fn prog_id(&self) -> String {
        format!(r"{}\{PROG_ID}", self.classes)
    }

    fn application(&self) -> String {
        format!(r"{}\Applications\{EXE_NAME}", self.classes)
    }

    fn capabilities(&self) -> String {
        format!(r"{}\{APP_NAME}\Capabilities", self.software)
    }

    fn registered_apps(&self) -> String {
        format!(r"{}\RegisteredApplications", self.software)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    NotRegistered,
    /// Зарегистрирован этот exe.
    Registered,
    /// Зарегистрирована другая копия (exe перенесли или запущен другой архив).
    OtherCopy(PathBuf),
}

/// Строка команды открытия: `"C:\…\markdowntown.exe" "%1"`.
fn open_command(exe: &Path) -> String {
    format!("\"{}\" \"%1\"", plain_path(exe).display())
}

/// Без префикса `\\?\` — его не понимают часть программ, читающих ассоциации.
fn plain_path(path: &Path) -> PathBuf {
    let s = path.as_os_str().to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => path.to_owned(),
    }
}

/// Путь exe из строки команды `"путь" "%1"`.
fn exe_from_command(command: &str) -> Option<PathBuf> {
    let rest = command.trim().strip_prefix('"')?;
    Some(PathBuf::from(&rest[..rest.find('"')?]))
}

pub fn status(loc: &Location, exe: &Path) -> Status {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let command: Option<String> = hkcu
        .open_subkey_with_flags(format!(r"{}\shell\open\command", loc.prog_id()), KEY_READ)
        .and_then(|key| key.get_value(""))
        .ok();
    match command {
        None => Status::NotRegistered,
        Some(c) if c.eq_ignore_ascii_case(&open_command(exe)) => Status::Registered,
        Some(c) => Status::OtherCopy(exe_from_command(&c).unwrap_or_else(|| c.into())),
    }
}

pub fn register(loc: &Location, exe: &Path) -> io::Result<()> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let exe = plain_path(exe);
    let command = open_command(&exe);
    let icon = format!("\"{}\",0", exe.display());

    // ProgID: тип файла, иконка, команда открытия.
    let (prog, _) = hkcu.create_subkey(loc.prog_id())?;
    prog.set_value("", &"Markdown document")?;
    prog.create_subkey("DefaultIcon")?.0.set_value("", &icon)?;
    prog.create_subkey(r"shell\open\command")?
        .0
        .set_value("", &command)?;

    // «Открыть с помощью» для расширений.
    for ext in EXTENSIONS {
        let (key, _) = hkcu.create_subkey(format!(r"{}\{ext}\OpenWithProgids", loc.classes))?;
        key.set_value(PROG_ID, &"")?;
    }

    // Сведения о приложении (имя в списках выбора, поддерживаемые типы).
    let (app, _) = hkcu.create_subkey(loc.application())?;
    app.set_value("FriendlyAppName", &APP_NAME)?;
    app.create_subkey(r"shell\open\command")?
        .0
        .set_value("", &command)?;
    let (types, _) = app.create_subkey("SupportedTypes")?;
    for ext in EXTENSIONS {
        types.set_value(ext, &"")?;
    }

    // Capabilities — чтобы приложение появилось в Параметрах → Приложения по умолчанию.
    let (caps, _) = hkcu.create_subkey(loc.capabilities())?;
    caps.set_value("ApplicationName", &APP_NAME)?;
    caps.set_value(
        "ApplicationDescription",
        &"Markdown editor with live preview and Mermaid diagrams",
    )?;
    let (assoc, _) = caps.create_subkey("FileAssociations")?;
    for ext in EXTENSIONS {
        assoc.set_value(ext, &PROG_ID)?;
    }
    let (apps, _) = hkcu.create_subkey(loc.registered_apps())?;
    apps.set_value(APP_NAME, &loc.capabilities())?;

    log::info!("registered file association for {}", exe.display());
    Ok(())
}

/// Удаляет всё, что записал [`register`]. Отсутствующие ключи — не ошибка.
pub fn unregister(loc: &Location) -> io::Result<()> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    for path in [
        loc.prog_id(),
        loc.application(),
        format!(r"{}\{APP_NAME}", loc.software),
    ] {
        ignore_missing(hkcu.delete_subkey_all(path))?;
    }
    for ext in EXTENSIONS {
        let path = format!(r"{}\{ext}\OpenWithProgids", loc.classes);
        if let Ok(key) = hkcu.open_subkey_with_flags(path, KEY_WRITE) {
            ignore_missing(key.delete_value(PROG_ID))?;
        }
    }
    if let Ok(key) = hkcu.open_subkey_with_flags(loc.registered_apps(), KEY_WRITE) {
        ignore_missing(key.delete_value(APP_NAME))?;
    }
    log::info!("removed file association");
    Ok(())
}

fn ignore_missing(result: io::Result<()>) -> io::Result<()> {
    match result {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// Сообщает Проводнику, что ассоциации изменились (иконки, «Открыть с помощью»).
pub fn notify_shell() {
    use windows_sys::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
    // SAFETY: для SHCNE_ASSOCCHANGED оба элемента должны быть нулевыми.
    unsafe {
        SHChangeNotify(
            SHCNE_ASSOCCHANGED as _,
            SHCNF_IDLIST,
            std::ptr::null(),
            std::ptr::null(),
        );
    }
}

/// Открывает Параметры → Приложения по умолчанию. В Windows 11 — сразу страницу
/// Markdowntown, в Windows 10 параметр игнорируется (общая страница).
pub fn open_default_apps_settings() -> io::Result<()> {
    let uri = format!("ms-settings:defaultapps?registeredAppUser={APP_NAME}");
    crate::links::shell_open(std::ffi::OsStr::new(&uri))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ветка реестра только для этого теста — настоящие ассоциации не трогаются.
    fn test_location(name: &str) -> (Location, String) {
        let root = format!(r"Software\Markdowntown-test-{name}-{}", std::process::id());
        let loc = Location {
            classes: format!(r"{root}\Classes"),
            software: root.clone(),
        };
        (loc, root)
    }

    #[test]
    fn register_status_unregister() {
        let (loc, root) = test_location("assoc");
        let exe = Path::new(r"C:\Tools\Markdowntown\markdowntown.exe");
        let other = Path::new(r"D:\Old\markdowntown.exe");

        assert_eq!(status(&loc, exe), Status::NotRegistered);
        register(&loc, exe).unwrap();
        assert_eq!(status(&loc, exe), Status::Registered);
        assert_eq!(status(&loc, other), Status::OtherCopy(exe.to_owned()));

        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let caps: String = hkcu
            .open_subkey(format!(r"{root}\RegisteredApplications"))
            .unwrap()
            .get_value(APP_NAME)
            .unwrap();
        assert_eq!(caps, format!(r"{root}\Markdowntown\Capabilities"));
        let openwith = hkcu
            .open_subkey(format!(r"{root}\Classes\.md\OpenWithProgids"))
            .unwrap();
        assert!(openwith.get_raw_value(PROG_ID).is_ok());

        unregister(&loc).unwrap();
        assert_eq!(status(&loc, exe), Status::NotRegistered);
        assert!(openwith.get_raw_value(PROG_ID).is_err());
        // Повторная отмена — не ошибка.
        unregister(&loc).unwrap();

        hkcu.delete_subkey_all(&root).unwrap();
    }

    #[test]
    fn command_line_roundtrip() {
        let exe = Path::new(r"\\?\C:\Program Files\Markdowntown\markdowntown.exe");
        let command = open_command(exe);
        assert_eq!(
            command,
            r#""C:\Program Files\Markdowntown\markdowntown.exe" "%1""#
        );
        assert_eq!(
            exe_from_command(&command).unwrap(),
            Path::new(r"C:\Program Files\Markdowntown\markdowntown.exe")
        );
    }
}
