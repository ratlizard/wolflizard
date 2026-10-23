//! A line-based control socket for the headless instruction-clock run.
//!
//! An input script fixes every key and click before the run starts, so a
//! harness that decides its next input from what the game shows has to stop
//! the run, read the save it wrote, and start again from that save. On
//! Cythera a restart costs about 200M instructions of loading before play
//! resumes, which makes a decision per move impractical. This socket keeps
//! one run alive and lets a client alternate between reading the machine and
//! running it.
//!
//! `SYSTEMLESS_CONTROL_SOCKET=<path>` binds a Unix socket and the run waits
//! for one client before it executes anything. The input script, if any,
//! still plays first; once its last event is delivered the run executes only
//! the instructions the client asks for. Each command is one line and gets
//! exactly one reply line, `ok ...` or `err <message>`:
//!
//! | command | effect |
//! |---|---|
//! | `run <n>` | execute `n` more instructions; the reply comes when they have run |
//! | `move <v> <h>`, `down <v> <h>`, `up <v> <h>`, `click <v> <h>` | mouse input, delivered now |
//! | `keydown <key> <ch>`, `keyup <key> <ch>`, `press <key> <ch>` | key input, delivered now |
//! | `peek <hex addr> <len>` | `ok <hex bytes>` read from guest memory |
//! | `poke <hex addr> <hex bytes>` | write guest memory |
//! | `dump <path>` | write all of guest RAM to a file |
//! | `shot <path>` | write the current frame as a PNG |
//! | `sync` | copy the guest's save files to the desktop store now |
//! | `status` | `ok <instructions> <guest tick> <halted 0/1>` |
//! | `quit` | end the run as if its instruction budget ran out |
//!
//! A client that disconnects leaves the run waiting for the next client,
//! so only `quit` ends it.
//!
//! Points are vertical first, as in the input script. The debug socket
//! (`--debug-socket`) covers breakpoints and registers but has no input
//! request, which is the half a player needs.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;

pub struct ControlSocket {
    listener: UnixListener,
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

/// What the run loop does after a command has been served.
pub enum ControlStep {
    /// Execute this many instructions, then serve again.
    Run(usize),
    /// Leave the run loop.
    Quit,
}

impl ControlSocket {
    /// Bind `path` and block until one client connects.
    pub fn accept(path: &Path) -> std::io::Result<Self> {
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        let listener = UnixListener::bind(path)?;
        eprintln!("[CONTROL] Listening on {}", path.display());
        let (reader, writer) = Self::wait_for_client(&listener)?;
        Ok(Self {
            listener,
            reader,
            writer,
        })
    }

    fn wait_for_client(
        listener: &UnixListener,
    ) -> std::io::Result<(BufReader<UnixStream>, UnixStream)> {
        let (stream, _) = listener.accept()?;
        eprintln!("[CONTROL] Client connected");
        let writer = stream.try_clone()?;
        Ok((BufReader::new(stream), writer))
    }

    /// The next command line. When a client disconnects, the run waits for
    /// the next one: reaching play from a saved game takes minutes, so a
    /// client that stops should not end the run. `None` means the socket
    /// itself failed.
    pub fn next_line(&mut self) -> Option<String> {
        loop {
            let mut line = String::new();
            match self.reader.read_line(&mut line) {
                Ok(n) if n > 0 => return Some(line.trim().to_string()),
                _ => {
                    eprintln!("[CONTROL] Client gone; waiting for another");
                    let (reader, writer) = Self::wait_for_client(&self.listener).ok()?;
                    self.reader = reader;
                    self.writer = writer;
                }
            }
        }
    }

    pub fn reply(&mut self, text: &str) {
        // A client that has gone is noticed at the next read.
        let _ = writeln!(self.writer, "{text}");
        let _ = self.writer.flush();
    }
}

/// A command parsed from one line, before the run loop applies it.
pub enum ControlCommand {
    Run(usize),
    Mouse { kind: MouseKind, v: i16, h: i16 },
    Key { kind: KeyKind, key: u8, ch: u8 },
    Peek { addr: u32, len: usize },
    Poke { addr: u32, bytes: Vec<u8> },
    Dump(String),
    Shot(String),
    Sync,
    Status,
    Quit,
}

#[derive(Clone, Copy)]
pub enum MouseKind {
    Move,
    Down,
    Up,
    Click,
}

#[derive(Clone, Copy)]
pub enum KeyKind {
    Down,
    Up,
    Press,
}

fn number<T: TryFrom<u64>>(token: Option<&&str>) -> Result<T, String> {
    let token = token.ok_or("missing argument")?;
    let value = match token.strip_prefix("0x") {
        Some(hex) => u64::from_str_radix(hex, 16),
        None => token.parse::<u64>(),
    }
    .map_err(|error| format!("bad number {token:?}: {error}"))?;
    T::try_from(value).map_err(|_| format!("{token:?} out of range"))
}

fn signed(token: Option<&&str>) -> Result<i16, String> {
    let token = token.ok_or("missing argument")?;
    token
        .parse::<i16>()
        .map_err(|error| format!("bad number {token:?}: {error}"))
}

fn hex_address(token: Option<&&str>) -> Result<u32, String> {
    let token = token.ok_or("missing address")?;
    let digits = token.trim_start_matches("0x").trim_start_matches('$');
    u32::from_str_radix(digits, 16).map_err(|_| format!("bad hex address {token:?}"))
}

pub fn parse_command(line: &str) -> Result<ControlCommand, String> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    let verb = *tokens.first().ok_or("empty command")?;
    let args = &tokens[1..];
    let mouse = |kind| -> Result<ControlCommand, String> {
        Ok(ControlCommand::Mouse {
            kind,
            v: signed(args.first())?,
            h: signed(args.get(1))?,
        })
    };
    let key = |kind| -> Result<ControlCommand, String> {
        Ok(ControlCommand::Key {
            kind,
            key: number(args.first())?,
            ch: number(args.get(1))?,
        })
    };
    match verb {
        "run" => Ok(ControlCommand::Run(number(args.first())?)),
        "move" => mouse(MouseKind::Move),
        "down" => mouse(MouseKind::Down),
        "up" => mouse(MouseKind::Up),
        "click" => mouse(MouseKind::Click),
        "keydown" => key(KeyKind::Down),
        "keyup" => key(KeyKind::Up),
        "press" => key(KeyKind::Press),
        "peek" => Ok(ControlCommand::Peek {
            addr: hex_address(args.first())?,
            len: number(args.get(1))?,
        }),
        "poke" => {
            let hex = args.get(1).ok_or("missing bytes")?;
            if hex.len() % 2 != 0 {
                return Err("poke bytes are an odd number of hex digits".into());
            }
            let bytes = (0..hex.len() / 2)
                .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16))
                .collect::<Result<Vec<u8>, _>>()
                .map_err(|_| "poke bytes are not hex".to_string())?;
            Ok(ControlCommand::Poke {
                addr: hex_address(args.first())?,
                bytes,
            })
        }
        "dump" => Ok(ControlCommand::Dump(
            args.first().ok_or("missing path")?.to_string(),
        )),
        "shot" => Ok(ControlCommand::Shot(
            args.first().ok_or("missing path")?.to_string(),
        )),
        "sync" => Ok(ControlCommand::Sync),
        "status" => Ok(ControlCommand::Status),
        "quit" => Ok(ControlCommand::Quit),
        other => Err(format!("unknown command {other:?}")),
    }
}

pub fn hex_string(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
