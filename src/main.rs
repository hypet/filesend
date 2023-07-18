// #![windows_subsystem = "windows"]

use std::collections::HashMap;
use std::path::PathBuf;
use std::{env, thread, cmp, hash, fs};
/*
GUI Windows-compatible application written in Rust with druid.
Application has functionality to choose a file and send it over network to another computer.
*/
use druid::im::{Vector, HashSet};
use druid::lens::Identity;
use home::home_dir;
use std::sync::{Arc, Mutex};
use tokio::net::{TcpListener, TcpStream};

use druid::widget::{Button, Container, Flex, Label, List, ProgressBar, TextBox, Scroll};
use druid::{
    AppDelegate, AppLauncher, Command, Data, DelegateCtx, Env, ExtEventSink,
    FileDialogOptions, Handled, Lens, Target, Widget, WidgetExt, WindowDesc, EventCtx, Selector, UnitPoint, LensExt, FileInfo,
};
use human_bytes::human_bytes;
use tokio::runtime::{Builder, Runtime};

use autodiscovery::TARGET_PEER_ADD_VAL_FN;
use autodiscovery::TARGET_PEER_REMOVE_VAL_FN;
use networking::send;
use networking::send_clipboard;
use networking::PROGRESSBAR_DTR_VAL_FN;
use networking::PROGRESSBAR_VAL_FN;
use networking::TRANSMITTITNG_FILENAME_VAL_FN;
use networking::{process_incoming, switch_transfer_state};

mod autodiscovery;
mod networking;

const RANDOM_PORT: u16 = 0;
const SET_CURRENT_TARGET: Selector<TargetPeer> = Selector::new("set_current_target_val_fn");
const ACCEPT_OPEN_FILE_TO_SEND: Selector<FileInfo> = Selector::new("accept_file_to_send");
const ACCEPT_OPEN_TARGET_DIR: Selector<FileInfo> = Selector::new("accept_target_dir");
const DEFAULT_RCV_DIR: [&str; 2] = ["Downloads", "filesend"];

#[derive(Debug, Clone, Data, Lens, Eq, PartialOrd, Ord)]
struct TargetPeer {
    hostname: String,
    ip: String,
    port: u16,
}

// Assuming one running instance of the application we take into account IP only
impl cmp::PartialEq for TargetPeer {
    fn eq(&self, other: &Self) -> bool {
        self.ip == other.ip
    }
}

impl hash::Hash for TargetPeer {
    fn hash<H: hash::Hasher>(&self, state: &mut H) {
        self.ip.hash(state);
    }
}

#[derive(Debug, Clone, Data, Lens)]
struct InnerState {
    target_dir: String,
}

#[derive(Debug, Clone, Data, Lens)]
struct AppState {
    file_name: String,
    host: String,
    port: String,
    progress: f64,
    dtr: String, // Data Transfer Rate
    rt: Arc<Runtime>,
    incoming_file_name: String,
    incoming_file_size: u64,
    target_list: HashSet<TargetPeer>, // Address list of available receivers
    connections: Arc<Mutex<HashMap<String, TcpStream>>>,
    outgoing_file_processing: Arc<Mutex<bool>>,
    target_dir: String,
    // innter: Arc<Mutex<InnerState>>,
}

impl AppState {
    fn new() -> AppState {
        AppState {
            file_name: "".into(),
            host: "".into(),
            port: "".into(),
            progress: 0.0,
            dtr: "".into(),
            rt: Arc::new(Builder::new_multi_thread().enable_all().build().unwrap()),
            incoming_file_name: "".into(),
            incoming_file_size: 0,
            target_list: HashSet::new(),
            connections: Arc::new(Mutex::from(HashMap::new())),
            outgoing_file_processing: Arc::new(Mutex::from(true)),
            target_dir: create_receiving_dir_if_needed().to_str().unwrap().into(),
            // innter: Arc::new(Mutex::new(InnerState {  })),
        }
    }
}

struct Delegate;

impl AppDelegate<AppState> for Delegate {
    fn command(
        &mut self,
        _ctx: &mut DelegateCtx,
        _target: Target,
        cmd: &Command,
        app_state: &mut AppState,
        _env: &Env,
    ) -> Handled {
        if let Some(file_info) = cmd.get(ACCEPT_OPEN_FILE_TO_SEND) {
            app_state.file_name = file_info.path().to_str().unwrap().into();
            return Handled::Yes;
        } else if let Some(dir_info) = cmd.get(ACCEPT_OPEN_TARGET_DIR) {
            // let mut lock = app_state.innter.lock().unwrap();
            app_state.target_dir = dir_info.path().to_str().unwrap().into();
            
            return Handled::Yes;
        } else if let Some(address) = cmd.get(TARGET_PEER_ADD_VAL_FN) {
            app_state.target_list.insert((*address).clone());
            return Handled::Yes;
        } else if let Some(address) = cmd.get(TARGET_PEER_REMOVE_VAL_FN) {
            app_state.target_list.remove(address);
            return Handled::Yes;
        } else if let Some(address) = cmd.get(SET_CURRENT_TARGET) {
            app_state.host = address.ip.clone();
            app_state.port = address.port.to_string();
            return Handled::Yes;
        } else if let Some(number) = cmd.get(PROGRESSBAR_VAL_FN) {
            app_state.progress = *number;
            return Handled::Yes;
        } else if let Some(dtr) = cmd.get(PROGRESSBAR_DTR_VAL_FN) {
            app_state.dtr = if *dtr > 0 {
                let mut dtr = human_bytes(*dtr);
                dtr.push_str("/s");
                dtr
            } else {
                "".into()
            };
            return Handled::Yes;
        } else if let Some(file_name) = cmd.get(TRANSMITTITNG_FILENAME_VAL_FN) {
            app_state.incoming_file_name = (*file_name).clone();
            return Handled::Yes;
        }

        Handled::No
    }
}

fn build_gui() -> impl Widget<AppState> {
    // File to send
    let open_dialog_options = FileDialogOptions::new()
        .name_label("Files or dirs to send")
        .accept_command(ACCEPT_OPEN_FILE_TO_SEND)
        .title("Files or dirs to send")
        .button_text("Open");
    let file_name_textbox = TextBox::new()
        .with_placeholder("File Name")
        .with_text_size(18.0)
        .fix_width(320.0)
        .align_left()
        .lens(AppState::file_name);
    let open_file_button = Button::new("Open").on_click(move |ctx, _, _| {
        ctx.submit_command(druid::commands::SHOW_OPEN_PANEL.with(open_dialog_options.clone()))
    })
        .fix_height(25.0);
    let file_row = Flex::row()
        .with_child(file_name_textbox)
        .with_child(open_file_button)
        .expand_width();
    
    // Target dir
    let target_dir_open_dialog_options = FileDialogOptions::new()
        .select_directories()
        .accept_command(ACCEPT_OPEN_TARGET_DIR)
        .name_label("Files or dirs to send")
        .title("Files or dirs to send")
        .button_text("Open");
    let open_target_dir_button = Button::new("Open").on_click(move |ctx, _, _| {
        ctx.submit_command(druid::commands::SHOW_OPEN_PANEL.with(target_dir_open_dialog_options.clone()))
    });
    let target_dir_textbox = TextBox::new()
        .with_placeholder("Target Directory")
        .with_text_size(18.0)
        .fix_width(320.0)
        .align_left()
        .lens(AppState::target_dir);
    let target_dir_row = Flex::row()
        .with_child(target_dir_textbox)
        .with_child(open_target_dir_button)
        .expand_width();

    // Host and port
    let host_textbox = TextBox::new()
        .with_placeholder("Host")
        .align_left()
        .lens(AppState::host);

    let port_textbox = TextBox::new()
        .with_placeholder("Port")
        .align_left()
        .lens(AppState::port);
    let address = Flex::row()
        .with_child(host_textbox)
        .with_spacer(10.0)
        .with_child(port_textbox);


    // Send buttons
    let send_clipboard_button = Button::new("Send Clipboard")
        .on_click(|_, data: &mut AppState, _| send_clipboard(data))
        .disabled_if(|data: &AppState, _| {
            data.host.is_empty() || data.port.is_empty()
        })
        .fix_height(30.0);
    let send_button = Button::new("Send")
        .on_click(|ctx, data: &mut AppState, _| send(data, ctx.get_external_handle()))
        .disabled_if(|data: &AppState, _| {
            data.file_name.is_empty() || (data.progress > 0.00 && data.progress < 1.0) || data.host.is_empty() || data.port.is_empty()
        })
        .fix_height(30.0);
    let stop_button = Button::new("Stop")
        .on_click(|_ctx, data: &mut AppState, _| switch_transfer_state(data, false))
        .disabled_if(|data: &AppState, _| {
            data.file_name.is_empty() || (data.progress == 0.00 || data.progress == 1.0) || data.host.is_empty() || data.port.is_empty()
        })
        .fix_height(30.0);
    let buttons_row = Flex::row()
        .with_child(send_button)
        .with_child(stop_button)
        .with_child(send_clipboard_button);

    let incoming_filename_label =
        Label::new(|data: &AppState, _: &_| data.incoming_file_name.clone())
            .expand_width()
            .center();
    let progress_bar = ProgressBar::new()
        .lens(AppState::progress)
        .expand_width()
        .center();

    let progress_label =
        Label::new(|data: &AppState, _: &_| format!("{:.2}%", data.progress * 100.0)).center();
    let progress_speed = Label::new(|data: &AppState, _: &_| data.dtr.clone()).align_right();
    let progress_row = Flex::row()
        .with_child(progress_label)
        .with_child(progress_speed);

    let main_column = Flex::column()
        .with_child(file_row)
        .with_spacer(5.0)
        .with_child(target_dir_row)
        .with_spacer(10.0)
        .with_child(address)
        .with_spacer(10.0)
        .with_child(buttons_row)
        .with_spacer(10.0)
        .with_child(incoming_filename_label)
        .with_spacer(10.0)
        .with_child(progress_bar)
        .with_spacer(10.0)
        .with_child(progress_row)
        .fix_width(400.0)
    ;

    let target_list = Flex::column().with_child(
        Scroll::new(
            List::new(|| {
                build_target_peer_item()
            })
                .fix_width(200.0)
        )
        .vertical()
        .lens(Identity.map(
            |d: &AppState| { 
                let v: Vector<TargetPeer> = d.target_list.clone().into_iter().collect();
                v
            }, 
            |_, _| {})
        )
    )
    .align_vertical(UnitPoint::TOP);

    let main_row = Flex::row()
        .with_child(main_column)
        .with_child(target_list);

    Container::new(main_row).center()
}

fn build_target_peer_item() -> impl Widget<TargetPeer> {
    Flex::row().with_child(
        Button::dynamic(|data: &String, _| data.clone()).lens(Identity.map(
            |d: &TargetPeer| { 
                format!("{}\n{}:{}", d.hostname, d.ip, d.port)
            }, 
            |_, _| {})
        )
            .on_click(|ctx: &mut EventCtx, data: &mut TargetPeer, _| {
                ctx.get_external_handle().submit_command(SET_CURRENT_TARGET, data.clone(), Target::Auto)
                    .expect("command failed to submit");
            })
            .fix_width(180.0)
    )
}

fn main() {
    let args: Vec<String> = env::args().collect();

    let app_state = AppState::new();

    let window = WindowDesc::new(build_gui())
        .title("File Transfer")
        .window_size((600.0, 280.0));

    let launcher = AppLauncher::with_window(window);

    let event_sink = launcher.get_external_handle();
    let mut port: String = RANDOM_PORT.to_string();
    if args.len() > 1 && !args[1].is_empty() {
        port = args[1].clone();
    }

    let app_state_clone = Arc::new(app_state.clone());

    thread::spawn(move || {
        start_tokio(app_state_clone, event_sink, port);
    });

    launcher
        .delegate(Delegate)
        .log_to_console()
        .launch(app_state)
        .expect("Failed to launch application");
}

#[tokio::main]
async fn start_tokio(app_state: Arc<AppState>, sink: ExtEventSink, port: String) {
    let listener = TcpListener::bind(format!("{}:{}", "0.0.0.0", port))
        .await
        .unwrap();

    let port = listener.local_addr().unwrap().port();
    let sink_clone = sink.clone();
    thread::spawn(move || {
        autodiscovery::start(sink_clone, port);
    });

    loop {
        let (mut socket, _) = listener.accept().await.unwrap();
        let sink_clone = sink.clone();
        if let Err(error) = process_incoming(app_state.clone(), sink_clone, &mut socket).await {
            eprintln!("Error while processing incoming client: {}", error);
        }
    }
}

fn create_receiving_dir_if_needed() -> PathBuf {
    let mut dir = home_dir().unwrap();
    dir.extend(DEFAULT_RCV_DIR);
    match fs::metadata(&dir) {
        Ok(_) => return dir,
        Err(_) => {
            println!("Creating dir: {:?}", dir);
            match fs::create_dir_all(&dir) {
                Ok(_) => return dir,
                Err(_) => {
                    eprintln!("Error while creating dir {:?}", dir);
                    PathBuf::new()
                }
            }
        },
    }
}