// #![windows_subsystem = "windows"]

use std::{env, thread};
/*
GUI Windows-compatible application written in Rust with druid.
Application has functionality to choose a file and send it over network to another computer.
*/
use std::fs::File;
use std::io::Read;
use std::str;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncWriteExt, self, BufReader, AsyncReadExt};
use tokio::net::{TcpStream, TcpListener};
use tokio::runtime::Builder;

use druid::widget::{Button, Container, Flex, Label, ProgressBar, TextBox};
use druid::{ commands,
    AppLauncher, AppDelegate, Command, DelegateCtx, Data, Env, FileDialogOptions, FileSpec, Handled, Lens, Target, Widget, WidgetExt, Window, WindowDesc,
};

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
            file_name: "".into(),
            host: "".into(),
            port: "".into(),
            progress: Arc::new(Mutex::from(0.0)),
            incoming_file_name: Arc::new(Mutex::from("".to_string())),
            incoming_file_size: Arc::new(Mutex::from(0)),
        }
    }

    fn send_file(&self) {
        let file_name = self.file_name.clone();
        let host = self.host.clone();
        let port = self.port.clone();

        let mut file = File::open(&file_name).unwrap();
        let file_size = file.metadata().unwrap().len();

        let rt = Builder::new_multi_thread().enable_all().build().unwrap();
        rt.block_on(async move {
            let mut stream = TcpStream::connect(format!("{}:{}", host, port))
                .await
                .unwrap();

            let mut file_name_buffer = file_name.into_bytes();
            let file_name_size = file_name_buffer.len() as u16;
            let file_name_size = file_name_size.to_be_bytes();
            let file_size_buf = file_size.to_be_bytes();
            let data_type = [0x01];
            let mut buf = vec![];
            buf.extend_from_slice(&data_type);
            buf.extend_from_slice(&file_name_size);
            buf.extend_from_slice(&file_size_buf);
            println!("Sending buf: {:?}", buf);

            stream.write_all(&buf).await.unwrap();

            let mut buf = [0; 1024];
            let mut total_sent: u64 = 0;

            while let Ok(n) = file.read(&mut buf) {
                if n == 0 {
                    break;
                }
                stream.write_all(&buf[0..n]).await.unwrap();
                total_sent += n as u64;
                let percentage = (total_sent as f64 / file_size as f64) * 100.0;
                *self.progress.lock().unwrap() = percentage;
            }
        });
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

    let data_type = reader.read_u8().await?;
    println!("data_type: {:?}", data_type);

    let file_name_size = reader.read_u16().await?;
    println!("file_name_size: {:?}", file_name_size);

    let file_size = reader.read_u64().await?;
    println!("file_size: {:?}", file_size);

    // let file_name_size = u16::from_be_bytes(file_name_size);
    // let file_size = u64::from_be_bytes(file_size);

    let mut file_name_buffer = vec![0u8; file_name_size as usize];
    reader.read_exact(&mut file_name_buffer).await?;
    if let Ok(file_name) = String::from_utf8(file_name_buffer) {
        match data_type {
            0x01 => {
                println!("Received file: {} ({} bytes)", file_name, file_size);
                // Handle file transfer
            },
            0x02 => {
                println!("Received text: {} ({} bytes)", file_name, file_size);
                // Handle text transfer
            },
            _ => {
                println!("Unknown data type: {}", data_type);
            },
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
        .allowed_types(vec![FileSpec::new("All Files", &["*.*"])])
        .name_label("File to send")
        .title("Choose a file to send")
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
