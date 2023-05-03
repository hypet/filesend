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
use tokio::io::{AsyncWriteExt, self, BufReader, AsyncReadExt};
use tokio::net::{TcpStream, TcpListener};
use tokio::runtime::Builder;

use druid::widget::{Button, Container, Flex, Label, ProgressBar, TextBox};
use druid::{ commands,
    AppLauncher, AppDelegate, Command, DelegateCtx, Data, Env, FileDialogOptions, FileSpec, Handled, Lens, Target, Widget, WidgetExt, Window, WindowDesc,
};

use walkdir::WalkDir;


#[derive(Clone, Data, Lens)]
struct GuiState {
    file_name: String,
    host: String,
    port: String,
    progress: Arc<Mutex<f64>>,
    incoming_file_name: Arc<Mutex<String>>,
    incoming_file_size: Arc<Mutex<i64>>,
}

impl GuiState {
    fn new() -> GuiState {
        GuiState {
            file_name: "C:\\pix\\2019".to_string(),
            host: "192.168.0.107".into(),
            port: "8080".into(),
            progress: Arc::new(Mutex::from(0.0)),
            incoming_file_name: Arc::new(Mutex::from("".to_string())),
            incoming_file_size: Arc::new(Mutex::from(0)),
        }
    }

    fn send_file(&self) {
        let path = Path::new(self.file_name.as_str());
        let host = self.host.clone();
        let port = self.port.clone();

        let rt = Builder::new_multi_thread().enable_all().build().unwrap();
        rt.block_on(async move {
            let mut stream = TcpStream::connect(format!("{}:{}", host, port))
                .await
                .unwrap();

            let path_parent = path.parent().unwrap();
            println!("> path_parent: {}", path_parent.to_str().unwrap());
            if path.is_dir() {
                let dir_root = path.file_name().unwrap();
                println!("> dir_root: {}", dir_root.to_str().unwrap());

                for entry in WalkDir::new(path) {
                    let entry = entry.unwrap();
                    let entry_path: &Path = entry.path().strip_prefix(path_parent).unwrap();
                    if entry.path().is_dir() {
                        println!("> Sending dir: entry_path: {}", entry_path.to_str().unwrap());
                        self.send_single_dir(&mut stream, entry_path.to_str().unwrap()).await;
                    } else {
                        let entry_full_path = entry.path().to_str().unwrap();
                        let mut full_path = PathBuf::from(self.file_name.clone());
                        full_path.push(entry.path().file_name().unwrap().to_str().unwrap());
                        self.send_single_file(&mut stream, entry_full_path, entry_path.to_str().unwrap()).await;
                    }
                }
                stream.write_u8(0x00).await;
            } else {
                let entry_path = path.strip_prefix(path_parent).unwrap();
                self.send_single_file(&mut stream, path.to_str().unwrap(), entry_path.to_str().unwrap()).await;
            }

            stream.write_u8(0).await;

            while let Ok(finish) = stream.read_u8().await {
                if finish == 0 {
                    println!("Transfer complete");
                } else {
                    println!("Received: {}", finish);
                }
            }
    
        });
    }

    async fn send_single_dir(&self, stream: &mut TcpStream, dir_name: &str) -> io::Result<()> {
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

    async fn send_single_file(&self, stream: &mut TcpStream, file_name: &str, relative_path: &str) -> io::Result<()> {
        println!("> Sending file: {}, relative_path: {}", file_name, relative_path);
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
            total_sent += n as u64;
            let percentage = (total_sent as f64 / file_size as f64) * 100.0;
            *self.progress.lock().unwrap() = percentage;
        }

        Ok(())
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

    let port = args[1].clone();
    thread::spawn(|| {
        start_tokio(port);
    });

    AppLauncher::with_window(window)
        .delegate(Delegate)
        .log_to_console()
        .launch(gui_state)
        .expect("Failed to launch application");
}

#[tokio::main]
async fn start_tokio(port: String) {
    let listener = TcpListener::bind(format!("{}:{}", "0.0.0.0", port)).await.unwrap();
    loop {
        let (mut socket, _) = listener.accept().await.unwrap();
        if let Err(error) = process_incoming(&mut socket).await {
            println!("Error: {}", error);
        }
    }
}

async fn process_incoming(socket: &mut TcpStream) -> io::Result<()> {
    loop {
        let data_type = socket.read_u8().await?;
        println!("< data_type: {:?}", data_type);

        match data_type {
            0x00 => {
                println!("< Transfer finished");
                socket.write_u8(0x0).await?;
                break; 
            }
            0x01 => {
                handle_file(socket).await?;
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

async fn handle_file(reader: &mut TcpStream) -> io::Result<()> {
    let file_name_size = reader.read_u16().await?;
    println!("< file_name_size: {:?}", file_name_size);
    let file_size = reader.read_u64().await?;
    println!("< file_size: {:?}", file_size);
    let mut file_name_buffer = vec![0u8; file_name_size as usize];
    reader.read_exact(&mut file_name_buffer).await?;

    if let Ok(file_name) = String::from_utf8(file_name_buffer) {
        println!("< Receiving file: {} ({} bytes)", file_name, file_size);
        let mut file = File::create(&file_name).unwrap();
        let mut total_received: u64 = 0;
        const BUF_SIZE: usize = 1024;
        let mut buf = [0u8; BUF_SIZE];

        if file_size < BUF_SIZE as u64 {
            let mut buf = vec![0u8; file_size as usize];
            let read_bytes = reader.read_exact(&mut buf).await?;
            println!("< Read small buf n = {}", read_bytes);
            if read_bytes == 0 {
                println!("< Can't read data anymore");
            }
            total_received += read_bytes as u64;
            file.write_all(&buf).unwrap();
        } else {
            while total_received < file_size {
                let read_bytes = reader.read_exact(&mut buf).await?;
                println!("< Read n = {}", read_bytes);
                if read_bytes == 0 {
                    eprintln!("< Can't read data anymore");
                    break;
                }
                total_received += read_bytes as u64;
                file.write_all(&buf)?;

                let bytes_left: i128 = file_size as i128 - total_received as i128;
                if bytes_left > 0 && bytes_left < BUF_SIZE as i128 {
                    let mut buf = vec![0u8; (file_size - total_received) as usize];
                    let n = reader.read_exact(&mut buf).await?;
                    println!("< Read finally n = {}", read_bytes);
                    file.write_all(&buf).unwrap();
                    total_received += n as u64;
                    break;
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
        .fix_width(300.0)
        .lens(GuiState::file_name.clone());
    let open_dialog_options = FileDialogOptions::new()
        .select_directories()
        .multi_selection()
        .name_label("Files or dirs to send")
        .title("Files or dirs to send")
        .button_text("Open");
    let open = Button::new("Open")
        .on_click(move |ctx, _, _| {
            ctx.submit_command(druid::commands::SHOW_OPEN_PANEL.with(open_dialog_options.clone()))
        });

    let host_textbox = TextBox::new().with_placeholder("Host").lens(GuiState::host);

    let port_textbox = TextBox::new().with_placeholder("Port").lens(GuiState::port);

    let send_button = Button::new("Send")
        .on_click(|_, data: &mut GuiState, _| data.send_file())
        .fix_height(30.0);

    let progress_label =
        Label::new(|data: &GuiState, _: &_| format!("{:.1}%", data.progress.lock().unwrap())).center();

    let main_column = Flex::column()
        .with_child(file_name_textbox)
        .with_child(open)
        .with_child(host_textbox)
        .with_child(port_textbox)
        .with_spacer(10.0)
        .with_child(send_button)
        .with_spacer(10.0)
        .with_child(progress_label);

    Container::new(
        Flex::column()
            .with_flex_child(main_column, 1.0)
            // .cross_axis_alignment(CrossAxisAlignment::Center)
            .main_axis_alignment(druid::widget::MainAxisAlignment::Center),
    )
    .center()
}
