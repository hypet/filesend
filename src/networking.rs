use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::thread;

use druid::{ExtEventSink, Selector, Target};
use msgpacker::prelude::*;
use tokio::io::{self, AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use walkdir::WalkDir;

use crate::AppState;

pub(crate) const PROGRESSBAR_VAL_FN: Selector<f64> = Selector::new("progressbar_val_fn");
pub(crate) const TRANSMITTITNG_FILENAME_VAL_FN: Selector<String> =
    Selector::new("transmitting_filename_val_fn");

pub enum DataType {
    FILE,
    DIRECTORY,
    TEXT_BUFFER,
}

impl DataType {
    pub fn to_u8(&self) -> u8 {
        match self {
            DataType::FILE => 0,
            DataType::DIRECTORY => 1,
            DataType::TEXT_BUFFER => 2,
        }
    }

    pub fn from_u8(value: u8) -> Option<DataType> {
        match value {
            0 => Some(DataType::FILE),
            1 => Some(DataType::DIRECTORY),
            2 => Some(DataType::TEXT_BUFFER),
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
        Some(DataType::FILE) => {
            handle_file(sink.clone(), socket, msg_size).await?;
        }
        Some(DataType::DIRECTORY) => {
            handle_dir(socket, msg_size).await?;
        }
        Some(DataType::TEXT_BUFFER) => {
            handle_text_buf(socket, msg_size).await?;
        }
        None => {
            eprintln!("< DataType is None");
        }
    }
    Ok(())
}

async fn handle_file(sink: ExtEventSink, socket: &mut TcpStream, msg_size: u64) -> io::Result<()> {
    println!("handling file");
    let mut buf = vec![0u8; msg_size as usize];
    socket.read_exact(&mut buf).await?;
    let (bytes, msg_file) = FileMeta::unpack(&buf).unwrap();
    println!("Decoded {} bytes for msgPack", bytes);

    let relative_path: PathBuf = msg_file.file_relative_path.iter().collect();

    println!("< Receiving file: {:?} ({} bytes)", relative_path, msg_file.file_size);
    sink.submit_command(
        TRANSMITTITNG_FILENAME_VAL_FN,
        Box::new(String::from(relative_path.to_str().unwrap())),
        Target::Auto,
    )
    .expect("command failed to submit");

    let mut file = File::create(&relative_path).unwrap();
    let mut total_received: u64 = 0;
    const BUF_SIZE: usize = 1024;
    let mut buf = [0u8; BUF_SIZE];
    let file_size = msg_file.file_size;

    if file_size < BUF_SIZE as u64 {
        let mut buf = vec![0u8; file_size as usize];
        let read_bytes = socket.read_exact(&mut buf).await?;
        if read_bytes == 0 {
            println!("< Can't read data");
        }
        total_received += read_bytes as u64;
        file.write_all(&buf).unwrap();

        sink.submit_command(PROGRESSBAR_VAL_FN, 1.0, Target::Auto)
            .expect("command failed to submit");
    } else {
        let mut counter: u8 = 0;
        while total_received < file_size {
            let read_bytes = socket.read_exact(&mut buf).await?;
            if read_bytes == 0 {
                eprintln!("< Can't read data");
                break;
            }
            total_received += read_bytes as u64;
            file.write_all(&buf)?;

            let bytes_left: i128 = file_size as i128 - total_received as i128;
            if bytes_left > 0 && bytes_left < BUF_SIZE as i128 {
                let mut buf = vec![0u8; (file_size - total_received) as usize];
                let n = socket.read_exact(&mut buf).await?;
                file.write_all(&buf).unwrap();
                total_received += n as u64;

                sink.submit_command(PROGRESSBAR_VAL_FN, 1.0, Target::Auto)
                    .expect("command failed to submit");
                break;
            }

            counter += 1;
            let percentage = total_received as f64 / file_size as f64;

            if counter % 100 == 0 {
                sink.submit_command(PROGRESSBAR_VAL_FN, percentage, Target::Auto)
                    .expect("command failed to submit");
                counter = 0;
            }
        }
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

    Ok(())
}

//////// Sending part

pub(crate) fn send(data: &mut AppState, sink: ExtEventSink) {
    let s = sink.clone();
    let path = data.file_name.clone();
    let host = data.host.clone();
    let port = data.port.clone();

    let rt = data.rt.clone();
    thread::spawn(move || {
        rt.block_on(async move {
            send_file(path, host, port, s.clone()).await;
            println!("Sender thread died");
        });
    });
}

async fn send_file(path: String, host: String, port: String, sink: ExtEventSink) {
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
                match send_single_dir(&mut stream, entry_path.to_path_buf()).await {
                    Ok(_) => {}
                    Err(e) => println!("Error: {}", e),
                }
            } else {
                let entry_full_path = entry.path().to_str().unwrap();
                let mut full_path = PathBuf::from(path.clone());
                full_path.push(entry.path().file_name().unwrap().to_str().unwrap());
                match send_single_file(
                    &mut stream,
                    entry_full_path,
                    entry_path.to_path_buf(),
                    sink.clone(),
                )
                .await
                {
                    Ok(_) => {}
                    Err(e) => println!("Error: {}", e),
                }
            }
        }
    } else {
        let entry_path = path.strip_prefix(path_parent).unwrap();
        match send_single_file(
            &mut stream,
            path.to_str().unwrap(),
            entry_path.to_path_buf(),
            sink,
        )
        .await
        {
            Ok(_) => {}
            Err(e) => println!("Error: {}", e),
        }
    }
}

async fn send_single_dir(stream: &mut TcpStream, dir_path_buf: PathBuf) -> io::Result<()> {
    let data = FileMeta {
        file_relative_path: path_to_vec(dir_path_buf),
        file_size: 0,
    };

    let mut buf = Vec::new();
    let msg_size = data.pack(&mut buf);

    stream.write_u64(msg_size as u64).await?;
    stream.write_u8(DataType::DIRECTORY.to_u8()).await?;
    stream.write(&buf).await?;

    Ok(())
}

async fn send_single_file(
    stream: &mut TcpStream,
    file_name: &str,
    relative_path: PathBuf,
    sink: ExtEventSink,
) -> io::Result<()> {
    println!(
        "> Sending file: {}, relative_path: {:?}",
        file_name, relative_path
    );

    sink.submit_command(PROGRESSBAR_VAL_FN, 0.0, Target::Auto)
        .expect("command failed to submit");
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
        file_size: file_size,
    };

    let mut buf = Vec::new();
    let msg_size = data.pack(&mut buf);
    println!("Packed msg size: {}", msg_size);

    stream.write_u64(msg_size as u64).await?;
    stream.write_u8(DataType::FILE.to_u8()).await?;
    stream.write(&buf).await?;

    let mut buf = [0; 1024];
    let mut total_sent: u64 = 0;
    let mut counter = 0;

    while let Ok(n) = file.read(&mut buf) {
        if n == 0 {
            break;
        }
        match stream.write_all(&buf[0..n]).await {
            Ok(_) => {}
            Err(e) => {
                println!("> Error: {}", e);
                break;
            }
        }
        counter += 1;
        total_sent += n as u64;
        let percentage = total_sent as f64 / file_size as f64;

        if counter % 100 == 0 {
            sink.submit_command(PROGRESSBAR_VAL_FN, percentage, Target::Auto)
                .expect("command failed to submit");
            counter = 0;
        }
    }
    sink.submit_command(PROGRESSBAR_VAL_FN, 1.0, Target::Auto)
        .expect("command failed to submit");

    Ok(())
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
