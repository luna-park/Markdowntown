//! Один экземпляр на каталог данных: файл, открытый из Проводника, когда Markdowntown уже
//! запущен, открывается вкладкой в запущенном окне, а не новым окном.
//!
//! Первый процесс создаёт именованный канал (`FILE_FLAG_FIRST_PIPE_INSTANCE` — заодно
//! и «замок»: второй процесс создать его не может) и слушает его в фоновом потоке.
//! Следующий процесс передаёт по каналу пути файлов, разрешает первому вывести окно
//! на передний план и завершается. Имя канала — по каталогу данных: копии программы
//! в разных папках независимы, а одна копия не делит сеанс и черновики между окнами.
//! Канал не принимает подключений по сети; писать в него может только тот же
//! пользователь (права канала по умолчанию).

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, OnceLock};

/// Пути, пришедшие от других процессов.
pub struct Inbox {
    rx: Option<mpsc::Receiver<Vec<PathBuf>>>,
    window: Arc<OnceLock<Window>>,
}

/// Окно программы. Показывает его поток канала, а не UI: у свёрнутого окна eframe
/// не вызывает `ui`, и запрос ждал бы, пока окно не развернут вручную.
struct Window {
    ctx: egui::Context,
    /// HWND (числом — его можно передать в поток канала).
    #[cfg_attr(not(windows), allow(dead_code))]
    handle: Option<isize>,
}

impl Inbox {
    /// Без канала: другие процессы сюда ничего не передают.
    pub fn none() -> Self {
        Self {
            rx: None,
            window: Arc::default(),
        }
    }

    /// Окно создано: с этого момента запросы показывают его и будят UI.
    pub fn attach(&self, cc: &eframe::CreationContext<'_>) {
        #[cfg(windows)]
        let handle = windows::hwnd(cc);
        #[cfg(not(windows))]
        let handle = None;
        let _ = self.window.set(Window {
            ctx: cc.egui_ctx.clone(),
            handle,
        });
    }

    /// Очередной запрос: пути файлов (пустой — просто показать окно).
    pub fn try_recv(&self) -> Option<Vec<PathBuf>> {
        self.rx.as_ref()?.try_recv().ok()
    }
}

pub enum Startup {
    /// Этот процесс — основной: запускать окно, принимать пути из `Inbox`.
    Primary(Inbox),
    /// Пути переданы уже запущенному экземпляру — этому процессу пора завершиться.
    Forwarded,
}

/// Проверяет, запущен ли уже экземпляр для каталога `key`; если да — передаёт ему `files`
/// (абсолютные пути). Если передать не удалось, процесс работает сам по себе.
pub fn start(key: &Path, files: &[PathBuf]) -> Startup {
    #[cfg(windows)]
    {
        windows::start(key, files)
    }
    #[cfg(not(windows))]
    {
        let _ = (key, files);
        Startup::Primary(Inbox::none())
    }
}

/// Имя канала для каталога данных: регистр пути в Windows не важен.
#[cfg_attr(not(windows), allow(dead_code))]
fn pipe_name(key: &Path) -> String {
    // FNV-1a: имя должно совпадать у разных версий программы (у `DefaultHasher`
    // алгоритм не зафиксирован).
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in key.to_string_lossy().to_lowercase().bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!(r"\\.\pipe\Markdowntown-{hash:016x}")
}

#[cfg(windows)]
mod windows {
    use std::ffi::{OsStr, OsString};
    use std::fs::File;
    use std::io::{self, Read, Write};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::path::{Path, PathBuf};
    use std::sync::mpsc;
    use std::sync::{Arc, OnceLock};
    use std::time::{Duration, Instant};

    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::Foundation::{
        ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED,
        GetLastError, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_INBOUND,
    };
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeServerProcessId,
        PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AllowSetForegroundWindow, IsIconic, SW_RESTORE, SetForegroundWindow, ShowWindow,
    };

    use super::{Inbox, Startup, Window, pipe_name};

    pub fn hwnd(cc: &eframe::CreationContext<'_>) -> Option<isize> {
        match cc.window_handle().ok()?.as_raw() {
            RawWindowHandle::Win32(win32) => Some(win32.hwnd.get()),
            _ => None,
        }
    }

    /// Окно — поверх остальных, свёрнутое — развернуть. Команды egui (`Minimized(false)`,
    /// `Focus`) не годятся: у свёрнутого окна они не выполняются, пока его не развернут.
    fn bring_to_front(handle: isize) {
        let hwnd = handle as HWND;
        // SAFETY: окно программы живёт, пока работает процесс (поток канала — его часть);
        // функции принимают окно другого потока.
        unsafe {
            if IsIconic(hwnd) != 0 {
                ShowWindow(hwnd, SW_RESTORE);
            }
            // Сработает, если запустивший нас процесс разрешил (`forward`).
            SetForegroundWindow(hwnd);
        }
    }

    /// Сколько ждать канал основного экземпляра (он может ещё запускаться).
    const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
    /// Больше путей за раз не бывает — защита от мусора в канале.
    const MAX_MESSAGE: u64 = 1 << 20;

    pub fn start(key: &Path, files: &[PathBuf]) -> Startup {
        let name = pipe_name(key);
        let wide: Vec<u16> = OsStr::new(&name).encode_wide().chain([0]).collect();
        match create_pipe(&wide, true) {
            Ok(first) => {
                let (tx, rx) = mpsc::channel();
                let window = Arc::new(OnceLock::new());
                let thread_window = Arc::clone(&window);
                let spawned = std::thread::Builder::new()
                    .name("single-instance".into())
                    .spawn(move || serve(first, &wide, &tx, &thread_window));
                match spawned {
                    Ok(_) => Startup::Primary(Inbox {
                        rx: Some(rx),
                        window,
                    }),
                    Err(_) => Startup::Primary(Inbox::none()),
                }
            }
            Err(e) if e.raw_os_error() == Some(ERROR_ACCESS_DENIED as i32) => {
                match forward(&name, files) {
                    Ok(()) => Startup::Forwarded,
                    // Основной экземпляр не отвечает (завершается) — работаем сами.
                    Err(_) => Startup::Primary(Inbox::none()),
                }
            }
            Err(_) => Startup::Primary(Inbox::none()),
        }
    }

    fn create_pipe(name: &[u16], first: bool) -> io::Result<OwnedHandle> {
        let mut open_mode = PIPE_ACCESS_INBOUND;
        if first {
            open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
        }
        // SAFETY: `name` — строка UTF-16 с нулём в конце; атрибуты безопасности — по умолчанию.
        let handle = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                open_mode,
                PIPE_TYPE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                0,
                64 * 1024,
                0,
                std::ptr::null(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: дескриптор только что создан и больше никому не принадлежит.
        Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
    }

    /// Фоновый поток основного экземпляра: принимает подключения по одному. Следующий
    /// экземпляр канала создаётся до закрытия текущего — иначе в промежутке новый
    /// процесс создал бы «первый» экземпляр и стал бы вторым основным.
    fn serve(
        first: OwnedHandle,
        name: &[u16],
        tx: &mpsc::Sender<Vec<PathBuf>>,
        window: &OnceLock<Window>,
    ) {
        let mut current = first;
        loop {
            // SAFETY: `current` — действующий дескриптор канала, ожидание синхронное.
            let connected =
                unsafe { ConnectNamedPipe(current.as_raw_handle(), std::ptr::null_mut()) };
            // SAFETY: читает код ошибки текущего потока.
            if connected == 0 && unsafe { GetLastError() } != ERROR_PIPE_CONNECTED {
                log::warn!("single instance: {}", io::Error::last_os_error());
            }
            let next = match create_pipe(name, false) {
                Ok(next) => next,
                Err(e) => {
                    log::error!("single instance: cannot create pipe: {e}");
                    return;
                }
            };
            let mut message = Vec::new();
            let read = File::from(current)
                .take(MAX_MESSAGE)
                .read_to_end(&mut message);
            match read {
                Ok(_) => {
                    if tx.send(decode(&message)).is_err() {
                        return; // UI завершился
                    }
                    // Окна ещё нет (программа запускается) — пути откроет первый кадр.
                    if let Some(window) = window.get() {
                        if let Some(handle) = window.handle {
                            bring_to_front(handle);
                        }
                        window.ctx.request_repaint();
                    }
                }
                Err(e) => log::warn!("single instance: cannot read request: {e}"),
            }
            current = next;
        }
    }

    /// Передаёт пути основному экземпляру и разрешает ему выйти на передний план
    /// (Windows не даёт фоновому процессу самому перехватить фокус).
    fn forward(name: &str, files: &[PathBuf]) -> io::Result<()> {
        let deadline = Instant::now() + CONNECT_TIMEOUT;
        loop {
            match File::options().write(true).open(name) {
                Ok(mut pipe) => {
                    let mut pid = 0;
                    // SAFETY: дескриптор канала действителен, пока жив `pipe`.
                    if unsafe { GetNamedPipeServerProcessId(pipe.as_raw_handle(), &mut pid) } != 0 {
                        // SAFETY: просто разрешение для процесса с этим PID.
                        unsafe { AllowSetForegroundWindow(pid) };
                    }
                    return pipe.write_all(&encode(files));
                }
                Err(e)
                    if Instant::now() < deadline
                        && matches!(
                            e.raw_os_error().map(|code| code as u32),
                            Some(ERROR_PIPE_BUSY | ERROR_FILE_NOT_FOUND)
                        ) =>
                {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Пути в UTF-16 (как их хранит Windows — без потерь), разделённые нулём.
    pub(super) fn encode(files: &[PathBuf]) -> Vec<u8> {
        let mut out = Vec::new();
        for file in files {
            for unit in file.as_os_str().encode_wide().chain([0]) {
                out.extend_from_slice(&unit.to_le_bytes());
            }
        }
        out
    }

    pub(super) fn decode(bytes: &[u8]) -> Vec<PathBuf> {
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&pair| u16::from_le_bytes(pair))
            .collect();
        units
            .split(|&unit| unit == 0)
            .filter(|path| !path.is_empty())
            .map(|path| PathBuf::from(OsString::from_wide(path)))
            .collect()
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn pipe_name_ignores_case_and_differs_by_folder() {
        let a = pipe_name(Path::new(r"C:\Apps\Markdowntown\data"));
        assert_eq!(a, pipe_name(Path::new(r"c:\apps\MARKDOWNTOWN\data")));
        assert_ne!(a, pipe_name(Path::new(r"D:\Markdowntown\data")));
        assert!(a.starts_with(r"\\.\pipe\Markdowntown-"));
    }

    #[test]
    fn message_round_trip() {
        let files = vec![
            PathBuf::from(r"C:\docs\заметки.md"),
            PathBuf::from(r"D:\a b\c.markdown"),
        ];
        assert_eq!(windows::decode(&windows::encode(&files)), files);
        assert!(windows::decode(&windows::encode(&[])).is_empty());
    }

    #[test]
    fn second_start_forwards_files_to_first() {
        let key = std::env::temp_dir().join(format!("mdt-single-{}", std::process::id()));
        let Startup::Primary(inbox) = start(&key, &[]) else {
            panic!("first start must be primary");
        };
        let files = vec![PathBuf::from(r"C:\x\one.md"), PathBuf::from(r"C:\y\two.md")];
        assert!(matches!(start(&key, &files), Startup::Forwarded));
        // Пустой запрос (exe запустили без файла) — тоже доходит: показать окно.
        assert!(matches!(start(&key, &[]), Startup::Forwarded));

        let mut received = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while received.len() < 2 && std::time::Instant::now() < deadline {
            match inbox.try_recv() {
                Some(paths) => received.push(paths),
                None => std::thread::sleep(std::time::Duration::from_millis(10)),
            }
        }
        assert_eq!(received, vec![files, Vec::new()]);
    }
}
