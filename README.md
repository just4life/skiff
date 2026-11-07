<p align="center">
  <img src="assets/icon.png" width="96" alt="Skiff">
</p>

<h1 align="center">Skiff</h1>

<p align="center">
  <b>Быстрый FTP / FTPS / SFTP клиент на чистом Rust с современным тёмным интерфейсом</b><br>
  Две панели, менеджер сайтов, очередь передач. Всё, что нужно от FileZilla, и ничего лишнего.
</p>

<p align="center">
  <a href="https://github.com/just4life/skiff/actions/workflows/build.yml"><img src="https://github.com/just4life/skiff/actions/workflows/build.yml/badge.svg" alt="build"></a>
  <a href="https://github.com/just4life/skiff/releases/latest"><img src="https://img.shields.io/github/v/release/just4life/skiff?color=6d8dff" alt="release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-GPL--3.0-8b5cf6" alt="license"></a>
  <img src="https://img.shields.io/badge/rust-2024-orange?logo=rust" alt="rust 2024">
  <img src="https://img.shields.io/badge/platform-Windows%20%7C%20Linux%20%7C%20macOS-lightgrey" alt="platforms">
</p>

<p align="center">
  <a href="https://github.com/just4life/skiff/releases/latest"><b>⬇ Скачать</b></a> ·
  <a href="#возможности">Возможности</a> ·
  <a href="#сборка">Сборка</a> ·
  <a href="README.en.md">English</a>
</p>

![Skiff](docs/screenshot-main.png)

<table>
  <tr>
    <td><img src="docs/screenshot-site-manager.png" alt="Менеджер сайтов"></td>
    <td><img src="docs/screenshot-transfer.png" alt="Очередь передач"></td>
  </tr>
  <tr>
    <td align="center">Менеджер сайтов с импортом из FileZilla</td>
    <td align="center">Параллельные передачи с прогрессом и скоростью</td>
  </tr>
  <tr>
    <td><img src="docs/screenshot-exists.png" alt="Файл уже существует"></td>
    <td><img src="docs/screenshot-certificate.png" alt="Проверка сертификата"></td>
  </tr>
  <tr>
    <td align="center">«Файл уже существует»: перезаписать, докачать, пропустить</td>
    <td align="center">Отпечатки ключей SSH и сертификатов TLS</td>
  </tr>
</table>

## Установка

Готовые сборки для Windows и Linux лежат на странице [Releases](https://github.com/just4life/skiff/releases/latest).
Это один исполняемый файл без установщика и зависимостей.

> Windows может показать предупреждение SmartScreen, потому что exe не подписан.
> Нажмите «Подробнее» и «Выполнить в любом случае».

## Возможности

- **Протоколы:** FTP, FTPS (явный `AUTH TLS` и неявный, порт 990), SFTP (SSH).
- **Аутентификация SFTP:** пароль, keyboard-interactive, ключи OpenSSH/PEM (ed25519, ECDSA, RSA), в том числе зашифрованные (пароль = парольная фраза).
- **Безопасность:** неизвестные ключи SSH и непроверенные TLS-сертификаты показываются с отпечатком SHA-256 — «доверять всегда / только сейчас / отмена», как в FileZilla. Изменившийся ключ подсвечивается как возможная атака.
- **Менеджер сайтов:** группы, поиск, дублирование, автосохранение, **импорт `sitemanager.xml` из FileZilla** (папки, пароли base64, удалённые/локальные папки, режим PASV/PORT).
- **Быстрое подключение:** строка вида `sftp://user@host:2222/path` разбирается автоматически.
- **Две панели:** локальная и удалённая, сортировка по колонкам, «естественная» сортировка имён, фильтр, история «назад», выбор с Ctrl/Shift, контекстное меню, горячие клавиши.
- **Передачи:** перетаскивание между панелями (и файлов из проводника ОС в удалённую панель), двойной щелчок, рекурсивные папки, очередь с прогрессом и скоростью, пауза, отмена, повтор, **N параллельных потоков** (у каждого своё соединение, браузинг не блокируется), **многопоточная передача одного большого файла** по частям через несколько соединений.
- **Многопоточная передача файла:** файлы от 16 МБ делятся на части (по умолчанию 4, до 8, «Настройки → Потоков на один файл»), каждая часть идёт своим соединением. Скачивание работает по FTP, FTPS и SFTP (REST + RETR / seek), загрузка на сервер — по SFTP (в FTP нет записи по смещению). Если сервер не дал лишнее соединение, его часть докачивает основное. При отмене или ошибке файл обрезается до целого начала, поэтому «докачать» потом работает правильно.
- **Если файл существует:** спросить / перезаписать / если новее / докачать (REST/APPE, seek в SFTP) / переименовать / пропустить, с «применить ко всем».
- **Файловые операции:** создание папок, переименование, удаление (папки — рекурсивно) локально и на сервере.
- **Надёжность:** keep-alive, автоматическое переподключение при обрыве, PASV с обходом NAT (как в FileZilla), EPSV для IPv6, MLSD с откатом на LIST, UTF-8.

## Параметры запуска

Как в FileZilla, к серверу можно подключиться сразу при запуске:

```bash
skiff sftp://root:пароль@192.168.1.10
skiff ftpes://user@ftp.example.com:21/www      # пароль спросит при запуске
skiff --site "Продакшн/Веб-сервер"             # сайт из менеджера сайтов
skiff ftp://user:pass@host --local D:\Проекты  # и открыть локальную папку
```

| Параметр | Что делает |
|---|---|
| `протокол://логин:пароль@сервер:порт/папка` | подключиться. Протоколы: `ftp`, `ftpes` (явный TLS), `ftps` (неявный TLS), `sftp` |
| `-c`, `--site <имя>` | подключиться к сайту из менеджера: `Имя`, `Группа/Имя` или `0/Группа/Имя` как в FileZilla |
| `-a`, `--local <папка>` | открыть локальную папку |
| `-h`, `--help` | справка |

Спецсимволы в пароле можно закодировать как в URL: `@` → `%40`, `:` → `%3A`, `/` → `%2F`, `%` → `%25`.
Если папки из адреса нет, откроется домашняя.

## Горячие клавиши

| Клавиша | Действие |
|---|---|
| `Enter` / двойной щелчок | открыть папку или передать файл |
| `Backspace` | на уровень вверх |
| `F2` | переименовать |
| `Del` | удалить |
| `F5` | обновить |
| `Ctrl+A` | выделить всё |
| `F12` | показать FPS и видеодрайвер |

## Сборка

Нужен Rust 1.88+ (`rustup`, edition 2024).

```bash
cargo build --release
./target/release/skiff
```

**Linux:** для GUI нужны библиотеки X11/Wayland и OpenGL, обычно уже установлены. На минимальных системах:
`sudo apt install libxkbcommon-x11-0 libxcursor1 libxrandr2 libxi6 libgl1`.

**Windows:** `cargo build --release` (MSVC) или кросс-сборка из Linux:
```bash
rustup target add x86_64-pc-windows-gnu && sudo apt install mingw-w64
cargo build --release --target x86_64-pc-windows-gnu
```

**macOS:** `cargo build --release`.

Рендеринг через OpenGL — работает в виртуалках, по RDP и на старых видеокартах.

## Где хранятся данные

`~/.config/skiff/` (Linux), `%APPDATA%\skiff\` (Windows), `~/Library/Application Support/skiff/` (macOS):

- `sites.json` — сайты. Пароли, как и в FileZilla по умолчанию, хранятся в **обфусцированном (base64), а не зашифрованном** виде; файл создаётся с правами `600`. Снимите «Запомнить пароль», чтобы вводить пароль при каждом подключении.
- `settings.json` — настройки окна и очереди.
- `trusted.json` — доверенные отпечатки ключей и сертификатов.

## Архитектура

```
src/
  main.rs          запуск окна (eframe/egui)
  app.rs           состояние приложения, события, раскладка
  config.rs        сайты, настройки, импорт FileZilla sitemanager.xml
  trust.rs         TOFU-хранилище ключей/сертификатов, rustls-верификатор
  session.rs       поток «браузера»: соединение для списка файлов и операций
  transfer.rs      очередь передач и пул потоков
  local.rs         локальная ФС
  remote/
    mod.rs         трейт RemoteFs + общие типы
    ftp.rs         FTP/FTPS (suppaftp + rustls)
    sftp.rs        SFTP (russh + russh-sftp, pure Rust)
  ui/
    pane.rs        панель файлов
    chrome.rs      тулбар, очередь, журнал, диалоги
    site_manager.rs
```

Как и в FileZilla, у панели сервера и у каждого потока передачи **своё соединение**. Все сетевые
операции идут в фоновых потоках, интерфейс никогда не ждёт сеть.

Заметка про FTPS: серверы (vsftpd, FileZilla Server, ProFTPD) требуют, чтобы каждое соединение
данных возобновляло TLS-сессию управляющего соединения. Билеты TLS 1.3 в rustls одноразовые, поэтому
FTPS работает через TLS 1.2 с «липким» кешем сессии — это надёжно переживает любое число передач.

## Тесты

```bash
cargo test                       # модульные тесты
# живые тесты против настоящих серверов:
SKIFF_IT="ftp://u:p@host/tmp ftpes://u:p@host/tmp ftps://u:p@host:990/tmp sftp://u:p@host/tmp" \
  cargo test live_roundtrip -- --ignored --nocapture
SKIFF_IT="…" cargo test segmented -- --ignored --nocapture   # многопоточная передача
```

Живой тест проверяет: загрузку, докачку (append), stat, листинг с кириллицей и пробелами, скачивание
целиком и с середины, отмену передачи с сохранением соединения, переименование, создание и удаление
папок и файлов.

## Связь с FileZilla

Skiff — не построчный перевод исходников FileZilla, а новая
реализация её основного функционала на Rust с опорой на поведение оригинала: формат
`sitemanager.xml`, обход NAT для PASV, отдельные соединения для передач, повторное использование
TLS-сессий в FTPS, диалог «файл существует», подтверждение ключей хоста.

| | FileZilla 3.71 | Skiff |
|---|---|---|
| Язык | C++ / wxWidgets | Rust / egui |
| Строк кода (без пустых) | ~89 тыс. клиент + ~35 тыс. libfilezilla | ~5,4 тыс. |
| Протоколы | FTP, FTPS, SFTP, Storj (S3 и др. — в Pro) | FTP, FTPS, SFTP |
| Размер | установщик + DLL | один файл ~19 МБ |

## Лицензия

GNU GPL v3 или новее (см. `LICENSE`) — совместимо с FileZilla (GPL v2 или новее) и с
Apache-2.0/MIT-зависимостями. Шрифт Inter — SIL Open Font License 1.1 (`assets/fonts/LICENSE-OFL.txt`).
