// #![windows_subsystem = "windows"]

use std::path::{Path, PathBuf};
use std::{env, thread, fs};
/*
GUI Windows-compatible application written in Rust with druid.
Application has functionality to choose a file and send it over network to another computer.
*/
use std::fs::File;
use std::io::{Read, Write};
use std::str;
use std::sync::{Arc, Mutex};
use rand::Rng;
use tokio::io::{AsyncWriteExt, self, AsyncReadExt};
use tokio::net::{TcpStream, TcpListener};
use tokio::runtime::{Builder, Runtime};

use druid::widget::{Button, Container, Flex, Label, ProgressBar, TextBox};
use druid::{ commands,
    AppLauncher, AppDelegate, Command, DelegateCtx, Data, Env, FileDialogOptions, Handled, Lens, Target, Widget, WidgetExt, WindowDesc, ExtEventSink, Selector,
};

use walkdir::WalkDir;

const PROGRESSBAR_VAL_FN: Selector<f64> = Selector::new("progressbar_val_fn");
const TRANSMITTITNG_FILENAME_VAL_FN: Selector<String> = Selector::new("transmitting_filename_val_fn");


#[derive(Clone, Data, Lens)]
struct GuiState {
    file_name: String,
    host: String,
    port: String,
    progress: f64,
    rt: Arc<Runtime>,
    incoming_file_name: String,
    incoming_file_size: Arc<Mutex<i64>>,
}

impl GuiState {
    fn new() -> GuiState {
        GuiState {
            file_name: "".to_string(),
            host: "".into(),
            port: "".into(),
            progress: 0.0,
            rt: Arc::new(Builder::new_multi_thread().enable_all().build().unwrap()),
            incoming_file_name: "".into(),
            incoming_file_size: Arc::new(Mutex::from(0)),
        }
    }
}


struct Delegate;

impl AppDelegate<GuiState> for Delegate {
    fn command(
        &mut self,
        _ctx: &mut DelegateCtx,
        _target: Target,
        cmd: &Command,
        data: &mut GuiState,
        _env: &Env,
    ) -> Handled {
        if let Some(file_info) = cmd.get(commands::OPEN_FILE) {
            let mut filename = String::new();
            if let Some(file) = file_info.path().to_str() {
                filename.push_str(file);
            }
            data.file_name = filename;
            return Handled::Yes;
        } else if let Some(number) = cmd.get(PROGRESSBAR_VAL_FN) {
            data.progress = *number;
            return Handled::Yes
        } else if let Some(file_name) = cmd.get(TRANSMITTITNG_FILENAME_VAL_FN) {
            data.incoming_file_name = (*file_name).clone();
            return Handled::Yes
        }

        Handled::No
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();

    let gui_state = GuiState::new();

    let window = WindowDesc::new(build_gui())
        .title("File Transfer")
        .window_size((400.0, 250.0));


    let launcher = AppLauncher::with_window(window);

    let event_sink = launcher.get_external_handle();
    let port = args[1].clone();
    thread::spawn(|| {
        start_tokio(event_sink, port);
    });

    launcher
        .delegate(Delegate)
        .log_to_console()
        .launch(gui_state)
        .expect("Failed to launch application");
}

#[tokio::main]
async fn start_tokio(sink: ExtEventSink, port: String) {
    let listener = TcpListener::bind(format!("{}:{}", "0.0.0.0", port)).await.unwrap();
    loop {
        let (mut socket, _) = listener.accept().await.unwrap();
        let sink_clone = sink.clone();
        if let Err(error) = process_incoming(sink_clone, &mut socket).await {
            println!("Error: {}", error);
        }
    }
}

async fn process_incoming(sink: ExtEventSink, socket: &mut TcpStream) -> io::Result<()> {
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
        println!("< Received dir: {}", dir_name);
        fs::create_dir_all(&dir_name).unwrap();
    }

    Ok(())
}

fn build_gui() -> impl Widget<GuiState> {
    let file_name_textbox = TextBox::new()
        .with_placeholder("File Name")
        .with_text_size(18.0)
        .expand_width()
        .align_left()
        .lens(GuiState::file_name.clone());
    let open_dialog_options = FileDialogOptions::new()
        .name_label("Files or dirs to send")
        .title("Files or dirs to send")
        .button_text("Open");
    let open = Button::new("Open")
        .on_click(move |ctx, _, _| {
            ctx.submit_command(druid::commands::SHOW_OPEN_PANEL.with(open_dialog_options.clone()))
        });

    let host_textbox = TextBox::new().with_placeholder("Host").align_left().lens(GuiState::host);

    let port_textbox = TextBox::new().with_placeholder("Port").align_left().lens(GuiState::port);

    let send_button = Button::new("Send")
        .on_click(|ctx, data: &mut GuiState, _| send(data, ctx.get_external_handle()))
        .fix_height(30.0);

    let incoming_filename_label =
        Label::new(|data: &GuiState, _: &_| data.incoming_file_name.clone()).expand_width().center();
    let progress_bar = ProgressBar::new().lens(GuiState::progress).expand_width().center();
    let progress_label =
        Label::new(|data: &GuiState, _: &_| format!("{:.2}%", data.progress * 100.0)).center();

    let address = Flex::row()
        .with_child(host_textbox)
        .with_spacer(10.0)
        .with_child(port_textbox);
    let main_column = Flex::column()
        .with_child(file_name_textbox)
        .with_child(open)
        .with_child(address)
        .with_spacer(10.0)
        .with_child(send_button)
        .with_spacer(10.0)
        .with_child(incoming_filename_label)
        .with_spacer(10.0)
        .with_child(progress_bar)
        .with_spacer(10.0)
        .with_child(progress_label);

    Container::new(main_column).center()
}

fn send(data: &mut GuiState, sink: ExtEventSink) {
    let s = sink.clone();
    let path = data.file_name.clone();
    let host = data.host.clone();
    let port = data.port.clone();

    let rt = data.rt.clone();
    thread::spawn(move || {
        rt.block_on(async move {
            // send_fake(s.clone()).await;
            send_file(path, host, port, s.clone()).await;
        });
    });
}

async fn send_fake(sink: ExtEventSink) {
    for i in 0..100 {
        sink.submit_command(PROGRESSBAR_VAL_FN, (i as f64 / 100.0 as f64) as f64, Target::Auto)
        .expect("command failed to submit");
        let delay_ms = rand::thread_rng().gen_range(100..1_000) as u64;
        tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
    }
}

async fn send_file(path: String, host: String, port: String, sink: ExtEventSink) {
    let path = Path::new(path.as_str());

    let mut stream = TcpStream::connect(format!("{}:{}", host, port))
        .await
        .unwrap();

    let path_parent = path.parent().unwrap();
    if path.is_dir() {
        let dir_root = path.file_name().unwrap();

        for entry in WalkDir::new(path) {
            let entry = entry.unwrap();
            let entry_path: &Path = entry.path().strip_prefix(path_parent).unwrap();
            if entry.path().is_dir() {
                println!("> Sending dir: entry_path: {}", entry_path.to_str().unwrap());
                send_single_dir(&mut stream, entry_path.to_str().unwrap()).await;
            } else {
                let entry_full_path = entry.path().to_str().unwrap();
                let mut full_path = PathBuf::from(path.clone());
                full_path.push(entry.path().file_name().unwrap().to_str().unwrap());
                send_single_file(&mut stream, entry_full_path, entry_path.to_str().unwrap(), sink.clone()).await;
            }
        }
        stream.write_u8(0x00).await;
    } else {
        let entry_path = path.strip_prefix(path_parent).unwrap();
        send_single_file(&mut stream, path.to_str().unwrap(), entry_path.to_str().unwrap(), sink).await;
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

async fn send_single_dir(stream: &mut TcpStream, dir_name: &str) -> io::Result<()> {
    let dir_name_buffer = dir_name.as_bytes();
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

async fn send_single_file(stream: &mut TcpStream, file_name: &str, relative_path: &str, sink: ExtEventSink) -> io::Result<()> {
    println!("> Sending file: {}, relative_path: {}", file_name, relative_path);

    sink.submit_command(PROGRESSBAR_VAL_FN, 0.0, Target::Auto)
        .expect("command failed to submit");
    sink.submit_command(TRANSMITTITNG_FILENAME_VAL_FN, file_name.to_string(), Target::Auto)
        .expect("command failed to submit");

    let mut file = File::open(file_name).unwrap();
    let file_size = file.metadata().unwrap().len();

    let file_name_buffer = relative_path.as_bytes();
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
