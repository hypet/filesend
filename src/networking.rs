use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Instant;

use clipboard::ClipboardContext;
use clipboard::ClipboardProvider;

use druid::{ExtEventSink, Selector, Target};
use msgpacker::prelude::*;
use tokio::io::{self, AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use walkdir::WalkDir;

use crate::AppState;

pub(crate) const PROGRESSBAR_VAL_FN: Selector<f64> = Selector::new("progressbar_val_fn");
pub(crate) const PROGRESSBAR_DTR_VAL_FN: Selector<u32> = Selector::new("progressbar_dtr_val_fn");
pub(crate) const TRANSMITTITNG_FILENAME_VAL_FN: Selector<String> = Selector::new("transmitting_filename_val_fn");
const UPDATE_PROGRESS_PERIOD_MS: u128 = 50;
const UPDATE_DTR_PERIOD_MS: u128 = 1000;
const TRANSMITTING_BUF_SIZE: usize = 1024;

pub enum DataType {
    File,
    Directory,
    TextBuffer,
}

impl DataType {
    pub fn to_u8(&self) -> u8 {
        match self {
            DataType::File => 0,
            DataType::Directory => 1,
            DataType::TextBuffer => 2,
        }
    }

    pub fn from_u8(value: u8) -> Option<DataType> {
        match value {
            0 => Some(DataType::File),
            1 => Some(DataType::Directory),
            2 => Some(DataType::TextBuffer),
            _ => None,
        }
    }
}

#[derive(MsgPacker)]
pub struct FileMeta {
    pub file_relative_path: Vec<String>,
    pub file_size: u64,
}

#[derive(MsgPacker)]
pub struct TextMeta {
    pub data: String,
}

//////// Receiving part

pub(crate) async fn process_incoming(sink: ExtEventSink, socket: &mut TcpStream) -> io::Result<()> {
    loop {
        println!("process_incoming");
        handle_msg_pack(socket, &sink).await?;
    }
}

async fn handle_msg_pack(socket: &mut TcpStream, sink: &ExtEventSink) -> io::Result<()> {
    let msg_size = socket.read_u64().await?;
    let msg_type = socket.read_u8().await?;
    println!("msg_size: {}, msg_type: {}", msg_size, msg_type);

    match DataType::from_u8(msg_type) {
        Some(DataType::File) => {
            handle_file(sink.clone(), socket, msg_size).await?;
        }
        Some(DataType::Directory) => {
            handle_dir(socket, msg_size).await?;
        }
        Some(DataType::TextBuffer) => {
            handle_text_buf(socket, msg_size).await?;
        }
        None => {
            eprintln!("< DataType is None");
        }
    }
    Ok(())
}

async fn handle_file(sink: ExtEventSink, socket: &mut TcpStream, msg_size: u64) -> io::Result<()> {
    println!("< handling file");
    let mut buf = vec![0u8; msg_size as usize];
    socket.read_exact(&mut buf).await?;
    let (bytes, msg_file) = FileMeta::unpack(&buf).unwrap();
    println!("< Decoded {} bytes for msgPack", bytes);

    let relative_path: PathBuf = msg_file.file_relative_path.iter().collect();

    println!(
        "< Receiving file: {:?} ({} bytes)",
        relative_path, msg_file.file_size
    );
    sink.submit_command(
        TRANSMITTITNG_FILENAME_VAL_FN,
        Box::new(String::from(relative_path.to_str().unwrap())),
        Target::Auto,
    )
    .expect("command failed to submit");

    let mut file = File::create(&relative_path).unwrap();
    let mut total_received: u64 = 0;
    let mut buf = [0u8; TRANSMITTING_BUF_SIZE];
    let file_size = msg_file.file_size;
    let mut last_update = Instant::now();

    if file_size < TRANSMITTING_BUF_SIZE as u64 {
        let mut buf = vec![0u8; file_size as usize];
        let read_bytes = socket.read_exact(&mut buf).await?;
        if read_bytes == 0 {
            eprintln!("< Can't read data");
        }
        total_received += read_bytes as u64;
        file.write_all(&buf).unwrap();

        update_progress_incoming(&sink, 1.0);
    } else {
        let mut counter: u32 = 0;
        while total_received < file_size {
            let read_bytes = socket.read_exact(&mut buf).await?;
            if read_bytes == 0 {
                eprintln!("< Can't read data");
                break;
            }
            total_received += read_bytes as u64;
            file.write_all(&buf)?;

            let bytes_left: i128 = file_size as i128 - total_received as i128;
            if bytes_left > 0 && bytes_left < TRANSMITTING_BUF_SIZE as i128 {
                let mut buf = vec![0u8; (file_size - total_received) as usize];
                let n = socket.read_exact(&mut buf).await?;
                file.write_all(&buf).unwrap();
                total_received += n as u64;

                update_progress_incoming(&sink, 1.0);
                break;
            }

            counter += 1;
            let percentage = total_received as f64 / file_size as f64;

            if counter % 100 == 0 && last_update.elapsed().as_millis() > UPDATE_PROGRESS_PERIOD_MS {
                update_progress_incoming(&sink, percentage);
                counter = 0;
                last_update = Instant::now();
            }
        }
        update_progress_incoming(&sink, 1.0);
    }
    println!("< Received {} bytes", total_received);

    Ok(())
}

async fn handle_dir(socket: &mut TcpStream, msg_size: u64) -> io::Result<()> {
    println!("handling dir");
    let mut buf = vec![0u8; msg_size as usize];
    socket.read_exact(&mut buf).await?;
    let (bytes, msg_file) = FileMeta::unpack(&buf).unwrap();
    println!("Decoded {} bytes for msgPack", bytes);

    let relative_path: PathBuf = msg_file.file_relative_path.iter().collect();

    println!("< Received dir: {:?}", relative_path);
    fs::create_dir_all(relative_path.as_path()).unwrap();

    Ok(())
}

async fn handle_text_buf(socket: &mut TcpStream, msg_size: u64) -> io::Result<()> {
    println!("handling text_buf");
    let mut buf = vec![0u8; msg_size as usize];
    socket.read_exact(&mut buf).await?;
    let (bytes, msg_text) = TextMeta::unpack(&buf).unwrap();
    println!("Decoded {} bytes for msgPack", bytes);

    println!("< Received text_buf: {:?}", msg_text.data);

    let mut ctx: ClipboardContext = ClipboardProvider::new().unwrap();
    match ctx.set_contents(msg_text.data) {
        Ok(_) => {},
        Err(e) => eprintln!("Error while setting clipboard content: {}", e),
    }

    Ok(())
}

//////// Sending part

pub(crate) fn switch_transfer_state(app_state: &mut AppState, val: bool) {
    let mut outgoing_flag = app_state.outgoing_file_processing.lock().unwrap();
    *outgoing_flag = val;
}

pub(crate) fn send_clipboard(app_state: &mut AppState) {
    let mut ctx: ClipboardContext = ClipboardProvider::new().unwrap();
    match ctx.get_contents() {
        Ok(content) => {
            println!("Clipboard content: {}", content);
            let host = app_state.host.clone();
            let port = app_state.port.clone();
            let rt = app_state.rt.clone();

            thread::spawn(move || {
                rt.block_on(async move {
                    match send_clipboard_inner(content, host, port).await {
                        Ok(_) => {},
                        Err(e) => eprintln!("Error while sending clipboard: {}", e),
                    }
                    println!("Clipboard sender thread died");
                });
            });
        }
        Err(e) => {
            eprintln!("Error while getting clipboard content: {}", e)
        }
    }
}

async fn send_clipboard_inner(
    content: String,
    host: String,
    port: String,
) -> io::Result<()> {
    let mut stream = TcpStream::connect(format!("{}:{}", host, port))
        .await
        .unwrap();

    let data = TextMeta {
        data: content
    };
    let mut buf = Vec::new();
    let msg_size = data.pack(&mut buf);

    stream.write_u64(msg_size as u64).await?;
    stream.write_u8(DataType::TextBuffer.to_u8()).await?;
    stream.write(&buf).await?;

    Ok(())
}

pub(crate) fn send(app_state: &mut AppState, sink: ExtEventSink) {
    switch_transfer_state(app_state, true);
    let s = sink.clone();
    let path = app_state.file_name.clone();
    let host = app_state.host.clone();
    let port = app_state.port.clone();

    let rt = app_state.rt.clone();
    let mut app_state = app_state.clone();
    thread::spawn(move || {
        rt.block_on(async move {
            send_file(&mut app_state, path, host, port, s.clone()).await;
            println!("Sender thread died");
        });
    });
}

async fn send_file(
    app_state: &mut AppState,
    path: String,
    host: String,
    port: String,
    sink: ExtEventSink,
) {
    let path = Path::new(path.as_str());

    let mut stream = TcpStream::connect(format!("{}:{}", host, port))
        .await
        .unwrap();

    let path_parent = path.parent().unwrap();
    if path.is_dir() {
        for entry in WalkDir::new(path) {
            let entry = entry.unwrap();
            let entry_path: &Path = entry.path().strip_prefix(path_parent).unwrap();
            if entry.path().is_dir() {
                println!(
                    "> Sending dir: entry_path: {}",
                    entry_path.to_str().unwrap()
                );
                match send_single_dir(&mut stream, entry_path.to_path_buf(), sink.clone()).await {
                    Ok(_) => {}
                    Err(e) => eprintln!("Error while sending dir: {}", e),
                }
            } else {
                let entry_full_path = entry.path().to_str().unwrap();
                let mut full_path = PathBuf::from(path.clone());
                full_path.push(entry.path().file_name().unwrap().to_str().unwrap());
                match send_single_file(
                    app_state,
                    &mut stream,
                    entry_full_path,
                    entry_path.to_path_buf(),
                    sink.clone(),
                )
                .await
                {
                    Ok(_) => {}
                    Err(e) => eprintln!("Error while sending file: {}", e),
                }
            }
        }
    } else {
        let entry_path = path.strip_prefix(path_parent).unwrap();
        match send_single_file(
            app_state,
            &mut stream,
            path.to_str().unwrap(),
            entry_path.to_path_buf(),
            sink,
        )
        .await
        {
            Ok(_) => {}
            Err(e) => eprintln!("Error while sending single file: {}", e),
        }
    }
}

async fn send_single_dir(
    stream: &mut TcpStream,
    dir_path_buf: PathBuf,
    sink: ExtEventSink,
) -> io::Result<()> {
    update_progress_incoming(&sink, 0.0);
    let data = FileMeta {
        file_relative_path: path_to_vec(dir_path_buf),
        file_size: 0,
    };

    let mut buf = Vec::new();
    let msg_size = data.pack(&mut buf);

    stream.write_u64(msg_size as u64).await?;
    stream.write_u8(DataType::Directory.to_u8()).await?;
    stream.write(&buf).await?;
    update_progress_incoming(&sink, 1.0);

    Ok(())
}

async fn send_single_file(
    app_state: &mut AppState,
    stream: &mut TcpStream,
    file_name: &str,
    relative_path: PathBuf,
    sink: ExtEventSink,
) -> io::Result<()> {
    println!(
        "> Sending file: {}, relative_path: {:?}",
        file_name, relative_path
    );

    update_progress_incoming(&sink, 0.0);
    sink.submit_command(
        TRANSMITTITNG_FILENAME_VAL_FN,
        file_name.to_string(),
        Target::Auto,
    )
    .expect("command failed to submit");

    let mut file = File::open(file_name).unwrap();
    let file_size = file.metadata().unwrap().len();

    let data = FileMeta {
        file_relative_path: path_to_vec(relative_path),
        file_size,
    };

    let mut buf = Vec::new();
    let msg_size = data.pack(&mut buf);
    println!("Packed msg size: {}", msg_size);

    stream.write_u64(msg_size as u64).await?;
    stream.write_u8(DataType::File.to_u8()).await?;
    stream.write(&buf).await?;

    let mut buf = [0; TRANSMITTING_BUF_SIZE];
    let mut total_sent: u64 = 0;
    let mut iter_counter: u32 = 0;
    let mut last_progress_update = Instant::now();
    let mut last_dtr_update = Instant::now();
    let mut sent_bytes_per_second: u32 = 0;

    while let Ok(n) = file.read(&mut buf) {
        let outgoing_flag = app_state.outgoing_file_processing.lock().unwrap();
        if !*outgoing_flag {
            break;
        }

        if n == 0 {
            break;
        }
        match stream.write_all(&buf[0..n]).await {
            Ok(_) => {}
            Err(e) => {
                eprintln!("> Error while writing buf into stream: {}", e);
                break;
            }
        }
        iter_counter += 1;
        total_sent += n as u64;
        let percentage = total_sent as f64 / file_size as f64;
        sent_bytes_per_second += n as u32;

        if iter_counter % 100 == 0
            && last_progress_update.elapsed().as_millis() > UPDATE_PROGRESS_PERIOD_MS
        {
            update_progress_incoming(&sink, percentage);
            last_progress_update = Instant::now();
            iter_counter = 0;
        }

        if last_dtr_update.elapsed().as_millis() >= UPDATE_DTR_PERIOD_MS {
            update_dtr(&sink, sent_bytes_per_second);
            last_dtr_update = Instant::now();
            sent_bytes_per_second = 0;
        }
    }
    update_progress_incoming(&sink, 1.0);
    update_dtr(&sink, 0);

    Ok(())
}

fn update_progress_incoming(sink: &ExtEventSink, value: f64) {
    sink.submit_command(PROGRESSBAR_VAL_FN, value, Target::Auto)
        .expect("command failed to submit");
}

fn update_dtr(sink: &ExtEventSink, value: u32) {
    sink.submit_command(PROGRESSBAR_DTR_VAL_FN, value, Target::Auto)
        .expect("command failed to submit");
}

fn path_to_vec(path: PathBuf) -> Vec<String> {
    path.components()
        .map(|comp| comp.as_os_str().to_string_lossy().into_owned())
        .collect()
}

#[cfg(test)]
mod test {
    use super::*;

    async fn setup() {}

    #[test]
    fn test_receive_file() {
        let file_name = "/tmp/filesend_123.tst";
        let tmp_file = File::create(file_name);
        assert_eq!(tmp_file.is_err(), false);
        fs::remove_file(file_name);
        // TODO complete test
    }
}
