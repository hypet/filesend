use std::borrow::Cow;
use std::fs::{File, self};
use std::io::{Write, Read};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::thread;

use druid::{ExtEventSink, Selector, Target};
use tokio::io::{AsyncWriteExt, self, AsyncReadExt};
use tokio::net::{TcpStream};

use walkdir::WalkDir;

use crate::GuiState;

pub(crate) const PROGRESSBAR_VAL_FN: Selector<f64> = Selector::new("progressbar_val_fn");
pub(crate) const TRANSMITTITNG_FILENAME_VAL_FN: Selector<String> = Selector::new("transmitting_filename_val_fn");


//////// Receiving part

pub(crate) async fn process_incoming(sink: ExtEventSink, socket: &mut TcpStream) -> io::Result<()> {
    loop {
        let data_type = socket.read_u8().await?;

        match data_type {
            0x00 => {
                println!("< Transfer finished");
                socket.write_u8(0x0).await?;
                break; 
            }
            0x01 => {
                handle_file(sink.clone(), socket).await?;
            },
            0x02 => {
                handle_dir(socket).await?;
            },
            0x03 => {
                let file_size = socket.read_u64().await?;
                println!("< text_buf_size: {:?}", file_size);
            },
            _ => {
                println!("< Unknown data type: {}", data_type);
            },
        }
    }

    Ok(())
}

async fn handle_file(sink: ExtEventSink, reader: &mut TcpStream) -> io::Result<()> {
    let file_name_size = reader.read_u16().await?;
    let file_size = reader.read_u64().await?;
    let mut file_name_buffer = vec![0u8; file_name_size as usize];
    reader.read_exact(&mut file_name_buffer).await?;

    if let Ok(file_name) = String::from_utf8(file_name_buffer) {
        println!("< Receiving file: {} ({} bytes)", file_name, file_size);
        sink.submit_command(TRANSMITTITNG_FILENAME_VAL_FN, file_name.to_string(), Target::Auto)
            .expect("command failed to submit");

        let mut file = File::create(&file_name).unwrap();
        let mut total_received: u64 = 0;
        const BUF_SIZE: usize = 1024;
        let mut buf = [0u8; BUF_SIZE];

        if file_size < BUF_SIZE as u64 {
            let mut buf = vec![0u8; file_size as usize];
            let read_bytes = reader.read_exact(&mut buf).await?;
            if read_bytes == 0 {
                println!("< Can't read data");
            }
            total_received += read_bytes as u64;
            file.write_all(&buf).unwrap();
        
            sink.submit_command(PROGRESSBAR_VAL_FN, 1.0, Target::Auto).expect("command failed to submit");
        } else {
            let mut counter: u8 = 0;
            while total_received < file_size {
                let read_bytes = reader.read_exact(&mut buf).await?;
                if read_bytes == 0 {
                    eprintln!("< Can't read data");
                    break;
                }
                total_received += read_bytes as u64;
                file.write_all(&buf)?;

                let bytes_left: i128 = file_size as i128 - total_received as i128;
                if bytes_left > 0 && bytes_left < BUF_SIZE as i128 {
                    let mut buf = vec![0u8; (file_size - total_received) as usize];
                    let n = reader.read_exact(&mut buf).await?;
                    file.write_all(&buf).unwrap();
                    total_received += n as u64;

                    sink.submit_command(PROGRESSBAR_VAL_FN, 1.0, Target::Auto).expect("command failed to submit");
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
    } else {
        eprintln!("< Invalid file name");
    }

    Ok(())
}

async fn handle_dir(reader: &mut TcpStream) -> io::Result<()> {
    let dir_name_size = reader.read_u16().await?;
    println!("< dir_name_size: {:?}", dir_name_size);
    let mut dir_name_buffer = vec![0u8; dir_name_size as usize];
    reader.read_exact(&mut dir_name_buffer).await?;
    if let Ok(dir_name) = String::from_utf8(dir_name_buffer) {
        let path_buf = PathBuf::from_str(&dir_name).unwrap();
        println!("< Received dir: {}, {:?}", dir_name, path_buf);
        fs::create_dir_all(path_buf.as_path()).unwrap();
    }

    Ok(())
}


//////// Sending part

pub(crate) fn send(data: &mut GuiState, sink: ExtEventSink) {
    let s = sink.clone();
    let path = data.file_name.clone();
    let host = data.host.clone();
    let port = data.port.clone();

    let rt = data.rt.clone();
    thread::spawn(move || {
        rt.block_on(async move {
            send_file(path, host, port, s.clone()).await;
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
                println!("> Sending dir: entry_path: {}", entry_path.to_str().unwrap());
                send_single_dir(&mut stream, entry_path.to_path_buf()).await;
            } else {
                let entry_full_path = entry.path().to_str().unwrap();
                let mut full_path = PathBuf::from(path.clone());
                full_path.push(entry.path().file_name().unwrap().to_str().unwrap());
                send_single_file(&mut stream, entry_full_path, entry_path.to_path_buf(), sink.clone()).await;
            }
        }
        stream.write_u8(0x00).await;
    } else {
        let entry_path = path.strip_prefix(path_parent).unwrap();
        send_single_file(&mut stream, path.to_str().unwrap(), entry_path.to_path_buf(), sink).await;
    }

    stream.write_u8(0).await;

    while let Ok(finish) = stream.read_u8().await {
        if finish == 0 {
            println!("Transfer complete");
            break;
        } else {
            println!("Received: {}", finish);
        }
    }
}

async fn send_single_dir(stream: &mut TcpStream, dir_path_buf: PathBuf) -> io::Result<()> {
    let encoded_dir: Cow<str> = paths_as_strings::encode_path(&dir_path_buf);
    let dir_name_buffer = encoded_dir.as_bytes();
    let dir_name_size = dir_name_buffer.len() as u16;
    
    stream.write_u8(0x02).await?;
    stream.write_u16(dir_name_size).await?;
    match stream.write_all(dir_name_buffer).await {
        Ok(_) => {}
        Err(e) => {
            println!("> Error: {}", e);
        }
    }

    Ok(())
}

async fn send_single_file(stream: &mut TcpStream, file_name: &str, relative_path: PathBuf, sink: ExtEventSink) -> io::Result<()> {
    println!("> Sending file: {}, relative_path: {:?}", file_name, relative_path);

    sink.submit_command(PROGRESSBAR_VAL_FN, 0.0, Target::Auto)
        .expect("command failed to submit");
    sink.submit_command(TRANSMITTITNG_FILENAME_VAL_FN, file_name.to_string(), Target::Auto)
        .expect("command failed to submit");

    let mut file = File::open(file_name).unwrap();
    let file_size = file.metadata().unwrap().len();

    let encoded_dir: Cow<str> = paths_as_strings::encode_path(&relative_path);
    let file_name_buffer = encoded_dir.as_bytes();
    let file_name_size = file_name_buffer.len() as u16;

    stream.write_u8(0x01).await?;
    stream.write_u16(file_name_size).await?;
    stream.write_u64(file_size).await?;
    stream.write_all(file_name_buffer).await?;

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
    sink.submit_command(PROGRESSBAR_VAL_FN, 1.0, Target::Auto).expect("command failed to submit");

    Ok(())
}
