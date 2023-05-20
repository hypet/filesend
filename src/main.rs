// #![windows_subsystem = "windows"]

use std::{env, thread};
/*
GUI Windows-compatible application written in Rust with druid.
Application has functionality to choose a file and send it over network to another computer.
*/
use std::sync::{Arc, Mutex};
use tokio::net::{TcpListener};

use druid::widget::{Button, Container, Flex, Label, ProgressBar, TextBox};
use druid::{ commands,
    AppLauncher, AppDelegate, Command, DelegateCtx, Data, Env, FileDialogOptions, Handled, Lens, Target, Widget, WidgetExt, WindowDesc, ExtEventSink,
};

use tokio::runtime::{Runtime, Builder};

use networking::TRANSMITTITNG_FILENAME_VAL_FN;
use networking::process_incoming;
use networking::send;

mod networking;

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
        } else if let Some(number) = cmd.get(networking::PROGRESSBAR_VAL_FN) {
            data.progress = *number;
            return Handled::Yes
        } else if let Some(file_name) = cmd.get(TRANSMITTITNG_FILENAME_VAL_FN) {
            data.incoming_file_name = (*file_name).clone();
            return Handled::Yes
        }

        Handled::No
    }
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
        .disabled_if(|data: &GuiState, _| data.file_name.is_empty() || (data.progress > 0.00 && data.progress < 1.0))
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
