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
        // let original_file_path = self.file_name.clone();
        let path = Path::new(self.file_name.as_str());
        // let path_buf = path.to_path_buf();
        // let file_name = path_buf.file_name().unwrap().to_str().unwrap();
        let host = self.host.clone();
        let port = self.port.clone();

        let rt = Builder::new_multi_thread().enable_all().build().unwrap();
        rt.block_on(async move {
            let mut stream = TcpStream::connect(format!("{}:{}", host, port))
                .await
                .unwrap();

            let path_parent = path.parent().unwrap();
            if path.is_dir() {
                for entry in WalkDir::new(path) {
                    let entry = entry.unwrap();
                    let entry_path = entry.path().strip_prefix(path_parent).unwrap();
                    if entry.path().is_dir() {
                        println!("Sending dir: {}, {}", entry_path.to_str().unwrap(), entry.path().file_name().unwrap().to_str().unwrap());
                        self.send_single_dir(&mut stream, entry_path.to_str().unwrap()).await;
                    } else {
                        println!("Path: {}", entry_path.to_str().unwrap());
                        let mut full_path = PathBuf::from(self.file_name.clone());
                        full_path.push(entry.path().file_name().unwrap().to_str().unwrap());
                        self.send_single_file(&mut stream, full_path.to_str().unwrap(), entry_path.to_str().unwrap()).await;
                        // self.send_single_file(&mut stream, entry.path().file_name().unwrap().to_str().unwrap()).await;
                    }
                }
            } else {
                let entry_path = path.strip_prefix(path_parent).unwrap();
                self.send_single_file(&mut stream, path.to_str().unwrap(), entry_path.to_str().unwrap()).await;
            }
    
        });
    }

    async fn send_single_dir(&self, stream: &mut TcpStream, dir_name: &str) {
        let file_name_buffer = dir_name.as_bytes();
        let file_name_size = file_name_buffer.len() as u16;
        let file_name_size = file_name_size.to_be_bytes();
        let file_size_buf = [0u8; 8];

        let data_type = [0x02];
        let mut buf = vec![];
        buf.extend_from_slice(&data_type);
        buf.extend_from_slice(&file_name_size);
        buf.extend_from_slice(&file_size_buf);
        println!("Sending dir meta buf: {:?}", buf);
        stream.write_all(&buf).await.unwrap();
    
        println!("Sending dir buf: {:?}", file_name_buffer);
        stream.write_all(&file_name_buffer).await.unwrap();
    }

    async fn send_single_file(&self, stream: &mut TcpStream, file_name: &str, relative_path: &str) {
        println!("Sending file: {}", file_name);
        let mut file = File::open(file_name).unwrap();
        let file_size = file.metadata().unwrap().len();
    
        let file_name_buffer = relative_path.as_bytes();
        let file_name_size = file_name_buffer.len() as u16;
        let file_name_size = file_name_size.to_be_bytes();
        let file_size_buf = file_size.to_be_bytes();
        let data_type = [0x01];
        let mut buf = vec![];
        buf.extend_from_slice(&data_type);
        buf.extend_from_slice(&file_name_size);
        buf.extend_from_slice(&file_size_buf);
        println!("Sending meta buf: {:?}", buf);
        stream.write_all(&buf).await.unwrap();
    
        println!("Sending buf: {:?}", file_name_buffer);
        stream.write_all(&file_name_buffer).await.unwrap();
    
        let mut buf = [0; 1024];
        let mut total_sent: u64 = 0;
    
        while let Ok(n) = file.read(&mut buf) {
            if n == 0 {
                break;
            }
            match stream.write_all(&buf[0..n]).await {
                Ok(_) => {}
                Err(e) => {
                    println!("Error: {}", e);
                    break;
                }
            }
            total_sent += n as u64;
            let percentage = (total_sent as f64 / file_size as f64) * 100.0;
            *self.progress.lock().unwrap() = percentage;
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
        let (socket, _) = listener.accept().await.unwrap();
        if let Err(error) = process_incoming(socket).await {
            println!("Error: {}", error);
        }
    }
}

async fn process_incoming(socket: TcpStream) -> io::Result<()> {
    let mut reader = BufReader::new(socket);
    loop {
        let data_type = reader.read_u8().await?;
        println!("data_type: {:?}", data_type);

        let file_name_size = reader.read_u16().await?;
        println!("file_name_size: {:?}", file_name_size);

        let file_size = reader.read_u64().await?;
        println!("file_size: {:?}", file_size);

        let mut file_name_buffer = vec![0u8; file_name_size as usize];
        reader.read_exact(&mut file_name_buffer).await?;
        println!("file_name_buffer: {:?}", file_name_buffer);
        if let Ok(file_name) = String::from_utf8(file_name_buffer) {
            println!("file_name: {:?}", file_name);
            match data_type {
                0x01 => {
                    println!("Receiving file: {} ({} bytes)", file_name, file_size);
                    let mut file = File::create(&file_name).unwrap();
                    let mut total_received: u64 = 0;
                    let mut buf = [0; 1024];
                    while let Ok(n) = reader.read(&mut buf).await {
                        if n == 0 {
                            break;
                        }
                        if total_received + n as u64 > file_size {
                            file.write_all(&buf[0..(file_size - total_received) as usize]).unwrap();
                            break;
                        } else {
                            file.write_all(&buf[0..n]).unwrap();
                            total_received += n as u64;
                        }
                        let percentage = (total_received as f64 / file_size as f64) * 100.0;
                        println!("Received {} bytes ({}%)", total_received, percentage);
                        file.flush().unwrap();
                    }
                },
                0x02 => {
                    println!("Received dir: {}", file_name);
                    // Handle dir transfer
                    fs::create_dir_all(&file_name).unwrap();
                },
                0x03 => {
                    let file_size = reader.read_u64().await?;
                    println!("text_buf_size: {:?}", file_size);
                    println!("Received text: {} ({} bytes)", file_name, file_size);
                    // Handle text transfer
                },
                _ => {
                    println!("Unknown data type: {}", data_type);
                },
            }
        }
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
