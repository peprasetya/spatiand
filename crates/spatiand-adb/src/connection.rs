//! A connection to a phone's adbd over Wireless debugging, with a key it has paired with, and
//! adb's `exec:` service on it: a command run as the shell user, its output read back.
//!
//! adb's transport: 24-byte messages (command, two arguments, the data's length and checksum,
//! and the command inverted), then the data. The computer says CNXN; a phone that wants TLS
//! answers STLS, the computer says STLS back, and both start TLS 1.3 on the same socket, in
//! which adbd checks the computer's key against those it has paired with. Then the phone's own
//! CNXN, and streams: OPEN a service, OKAY, WRTE each way (each WRTE answered OKAY), CLSE.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use crate::key::AdbKey;

const CNXN: u32 = 0x4e58_4e43;
const STLS: u32 = 0x534c_5453;
const AUTH: u32 = 0x4854_5541;
const OPEN: u32 = 0x4e45_504f;
const OKAY: u32 = 0x5941_4b4f;
const CLSE: u32 = 0x4553_4c43;
const WRTE: u32 = 0x4554_5257;
/// The protocol's version from which checksums are no longer looked at.
const VERSION: u32 = 0x0100_0001;
const STLS_VERSION: u32 = 0x0100_0000;
const MAX_DATA: u32 = 1024 * 1024;

pub struct Connection {
    tls: rustls::StreamOwned<rustls::ClientConnection, TcpStream>,
    /// What the phone said it is.
    pub banner: String,
    next_stream: u32,
}

struct Message {
    command: u32,
    arg0: u32,
    arg1: u32,
    data: Vec<u8>,
}

impl Connection {
    /// Connect to adbd's TLS port at `address` with `key`, which it must have paired with.
    pub fn connect(address: SocketAddr, key: &AdbKey) -> Result<Connection, String> {
        let mut socket = TcpStream::connect_timeout(&address, Duration::from_secs(5))
            .map_err(|e| format!("could not reach adb at {address}: {e}"))?;
        socket.set_read_timeout(Some(Duration::from_secs(20))).ok();
        socket.set_write_timeout(Some(Duration::from_secs(20))).ok();
        socket.set_nodelay(true).ok();
        write(&mut socket, CNXN, VERSION, MAX_DATA, b"host::features=shell_v2,cmd")?;
        let answer = read(&mut socket)?;
        match answer.command {
            STLS => {}
            AUTH => return Err("this adb asks for the old authentication, not TLS: not Wireless debugging".into()),
            other => return Err(format!("adb answered {other:#x}, not STLS")),
        }
        write(&mut socket, STLS, STLS_VERSION, 0, &[])?;
        let mut tls = crate::tls::connect(key, address.ip(), socket)
            .map_err(|e| format!("{e} (has the phone paired with Spatiand?)"))?;
        let hello = read(&mut tls).map_err(|e| format!("{e} (has the phone paired with Spatiand?)"))?;
        if hello.command != CNXN {
            return Err(format!("adb answered {:#x}, not CNXN", hello.command));
        }
        Ok(Connection { tls, banner: String::from_utf8_lossy(&hello.data).into_owned(), next_stream: 1 })
    }

    /// Run `command` as the shell user, through `sh -c`, and return what it wrote once it ends.
    pub fn exec(&mut self, command: &str) -> Result<Vec<u8>, String> {
        let local = self.next_stream;
        self.next_stream += 1;
        let mut service = format!("exec:{command}").into_bytes();
        service.push(0);
        write(&mut self.tls, OPEN, local, 0, &service)?;
        let mut output = Vec::new();
        let mut remote = 0;
        loop {
            let m = read(&mut self.tls)?;
            if m.arg1 != local {
                continue;
            }
            match m.command {
                OKAY => remote = m.arg0,
                WRTE => {
                    remote = m.arg0;
                    output.extend_from_slice(&m.data);
                    write(&mut self.tls, OKAY, local, remote, &[])?;
                }
                CLSE => {
                    if remote != 0 {
                        let _ = write(&mut self.tls, CLSE, local, remote, &[]);
                    }
                    return Ok(output);
                }
                _ => {}
            }
        }
    }
}

fn write(to: &mut impl Write, command: u32, arg0: u32, arg1: u32, data: &[u8]) -> Result<(), String> {
    let mut message = Vec::with_capacity(24 + data.len());
    for word in [command, arg0, arg1, data.len() as u32, 0, command ^ 0xffff_ffff] {
        message.extend_from_slice(&word.to_le_bytes());
    }
    message.extend_from_slice(data);
    to.write_all(&message).and_then(|_| to.flush()).map_err(|e| format!("could not send to adb: {e}"))
}

fn read(from: &mut impl Read) -> Result<Message, String> {
    let mut header = [0u8; 24];
    from.read_exact(&mut header).map_err(|e| format!("adb closed the connection ({e})"))?;
    let word = |i: usize| u32::from_le_bytes(header[4 * i..4 * i + 4].try_into().unwrap());
    let (command, arg0, arg1, length, magic) = (word(0), word(1), word(2), word(3), word(5));
    if magic != command ^ 0xffff_ffff || length > MAX_DATA {
        return Err(format!("not an adb message: {header:02x?}"));
    }
    let mut data = vec![0u8; length as usize];
    from.read_exact(&mut data).map_err(|e| format!("adb closed the connection ({e})"))?;
    Ok(Message { command, arg0, arg1, data })
}
