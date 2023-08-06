/*
GUI Windows-compatible application written in Rust with druid.
Application has functionality to choose a file and send it over network to another computer.
*/

// #![windows_subsystem = "windows"]

use std::collections::HashMap;
use std::path::PathBuf;
use std::{cmp, env, fs, hash, thread};
use druid::im::{HashSet, Vector};
use druid::lens::Identity;
use home::home_dir;
use tokio::sync::watch::{Sender, self, Receiver};
use std::sync::{Arc, Mutex};
use tokio::net::{TcpListener, TcpStream};
use tray_item::{IconSource, TrayItem};

use druid::widget::{Button, Container, Flex, Label, List, ProgressBar, Scroll, TextBox, LineBreaking};
use druid::{
    AppDelegate, AppLauncher, Command, Data, DelegateCtx, Env, EventCtx, ExtEventSink,
    FileDialogOptions, FileInfo, Handled, Lens, LensExt, Selector, Target, UnitPoint, Widget,
    WidgetExt, WindowDesc, Insets,
};
use human_bytes::human_bytes;
use tokio::runtime::{Builder, Runtime};

use autodiscovery::TARGET_PEER_ADD_VAL_FN;
use autodiscovery::TARGET_PEER_REMOVE_VAL_FN;
use networking::send;
use networking::send_clipboard;
use networking::switch_transfer_state;
use networking::PROGRESSBAR_DTR_VAL_FN;
use networking::PROGRESSBAR_VAL_FN;
use networking::TRANSMITTITNG_FILENAME_VAL_FN;
use networking::DataReceiver;

mod autodiscovery;
mod networking;

const RANDOM_PORT: u16 = 0;
const SET_CURRENT_PEER: Selector<TargetPeer> = Selector::new("set_current_target_val_fn");
const ACCEPT_OPEN_FILE_TO_SEND: Selector<FileInfo> = Selector::new("accept_file_to_send");
const ACCEPT_OPEN_DOWNLOAD_DIR: Selector<FileInfo> = Selector::new("accept_download_dir");
const DEFAULT_RCV_DIR: [&str; 2] = ["Downloads", "filesend"];

const GUI_TEXT_SIZE: f64 = 14.0;

#[derive(Debug, Clone, Data, Lens, Eq, PartialOrd, Ord)]
struct TargetPeer {
    hostname: String,
    ip: String,
    port: u16,
}

// IP address is taken into account, assuming one running instance of the application
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
    download_dir: String,
    download_dir_sender: Arc<Sender<String>>,
    download_dir_rcvr: Arc<Receiver<String>>,
}

impl AppState {
    fn new() -> AppState {
        let download_dir = create_receiving_dir_if_needed().to_str().unwrap().to_string();
        let (download_dir_tx, download_dir_rx) = watch::channel(download_dir.clone());
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
            download_dir: download_dir,
            download_dir_sender: Arc::from(download_dir_tx),
            download_dir_rcvr: Arc::from(download_dir_rx),
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
        } else if let Some(dir_info) = cmd.get(ACCEPT_OPEN_DOWNLOAD_DIR) {
            app_state.download_dir = dir_info.path().to_str().unwrap().to_string();
            match app_state.download_dir_sender.send(app_state.download_dir.clone()) {
                Ok(_) => {},
                Err(e) => eprintln!("Error while sending download_dir value: {}", e),
            }
            return Handled::Yes;
        } else if let Some(address) = cmd.get(TARGET_PEER_ADD_VAL_FN) {
            app_state.target_list.insert((*address).clone());
            return Handled::Yes;
        } else if let Some(address) = cmd.get(TARGET_PEER_REMOVE_VAL_FN) {
            app_state.target_list.remove(address);
            return Handled::Yes;
        } else if let Some(address) = cmd.get(SET_CURRENT_PEER) {
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
        .with_placeholder("File or Directory to send")
        .with_text_size(GUI_TEXT_SIZE)
        .fix_width(320.0)
        .align_left()
        .lens(AppState::file_name);
    let open_file_button = Button::new("Open")
        .on_click(move |ctx, _, _| {
            ctx.submit_command(druid::commands::SHOW_OPEN_PANEL.with(open_dialog_options.clone()))
        })
        .fix_height(25.0);
    let file_row = Flex::row()
        .with_child(file_name_textbox)
        .with_child(open_file_button)
        .expand_width();

    // Download dir
    let download_dir_open_dialog_options = FileDialogOptions::new()
        .select_directories()
        .accept_command(ACCEPT_OPEN_DOWNLOAD_DIR)
        .name_label("Files or dirs to send")
        .title("Files or dirs to send")
        .button_text("Open");
    let open_download_dir_button = Button::new("Open").on_click(move |ctx, _, _| {
        ctx.submit_command(
            druid::commands::SHOW_OPEN_PANEL.with(download_dir_open_dialog_options.clone()),
        )
    });

    let download_dir_label = Label::new(|data: &AppState, _: &_| data.download_dir.clone())
        .with_line_break_mode(LineBreaking::WordWrap)
        .with_text_size(GUI_TEXT_SIZE)
        .fix_width(320.0)
        .align_left();

    let download_dir_row = Flex::row()
        .with_child(download_dir_label)
        .with_child(open_download_dir_button)
        .expand_width();

    // Host and port
    let host_textbox = TextBox::new()
        .with_placeholder("Host")
        .with_text_size(GUI_TEXT_SIZE)
        .align_left()
        .lens(AppState::host);

    let port_textbox = TextBox::new()
        .with_placeholder("Port")
        .with_text_size(GUI_TEXT_SIZE)
        .align_left()
        .lens(AppState::port);
    let address_row = Flex::row()
        .with_child(host_textbox)
        .with_spacer(10.0)
        .with_child(port_textbox);

    // Send buttons
    let send_clipboard_button = Button::new("Send Clipboard")
        .on_click(|_, data: &mut AppState, _| send_clipboard(data))
        .disabled_if(|data: &AppState, _| data.host.is_empty() || data.port.is_empty())
        .fix_height(30.0);
    let send_button = Button::new("Send")
        .on_click(|ctx, data: &mut AppState, _| send(data, ctx.get_external_handle()))
        .disabled_if(|data: &AppState, _| {
            data.file_name.is_empty()
                || (data.progress > 0.00 && data.progress < 1.0)
                || data.host.is_empty()
                || data.port.is_empty()
        })
        .fix_height(30.0);
    let stop_button = Button::new("Stop")
        .on_click(|_ctx, data: &mut AppState, _| switch_transfer_state(data, false))
        .disabled_if(|data: &AppState, _| {
            data.file_name.is_empty()
                || (data.progress == 0.00 || data.progress == 1.0)
                || data.host.is_empty()
                || data.port.is_empty()
        })
        .fix_height(30.0);
    let buttons_row = Flex::row()
        .with_child(send_button)
        .with_child(stop_button)
        .with_child(send_clipboard_button);

    let incoming_filename_label = Label::new(|data: &AppState, _: &_| data.incoming_file_name.clone())
        .with_line_break_mode(LineBreaking::WordWrap)
        .with_text_size(GUI_TEXT_SIZE)
        .fix_width(400.0)
        .fix_height(45.0)
        .align_vertical(UnitPoint::BOTTOM)
        .center();
    let progress_bar = ProgressBar::new()
        .lens(AppState::progress)
        .expand_width()
        .center();

    let progress_label = Label::new(|data: &AppState, _: &_| format!("{:.2}%", data.progress * 100.0))
        .with_text_size(GUI_TEXT_SIZE)
        .center();
    let progress_speed = Label::new(|data: &AppState, _: &_| data.dtr.clone())
        .with_text_size(GUI_TEXT_SIZE)
        .align_right();
    let progress_row = Flex::row()
        .with_child(progress_label)
        .with_child(progress_speed);

    let main_column = Flex::column()
        .with_child(file_row)
        .with_spacer(5.0)
        .with_child(download_dir_row)
        .with_spacer(10.0)
        .with_child(address_row)
        .with_spacer(10.0)
        .with_child(buttons_row)
        .with_spacer(10.0)
        .with_child(incoming_filename_label)
        .with_spacer(10.0)
        .with_child(progress_bar)
        .with_spacer(10.0)
        .with_child(progress_row)
        .padding(Insets::uniform_xy(5.0, 5.0))
        .fix_width(400.0);

    let target_list = Flex::column()
        .with_child(
            Scroll::new(List::new(|| build_target_peer_item()).fix_width(200.0))
                .vertical()
                .lens(Identity.map(
                    |d: &AppState| {
                        let v: Vector<TargetPeer> = d.target_list.clone().into_iter().collect();
                        v
                    },
                    |_, _| {},
                )),
        )
        .with_spacer(1.0)
        .align_vertical(UnitPoint::TOP);

    let main_row = Flex::row().with_child(main_column).with_child(target_list);

    Container::new(main_row).center()
}

fn build_target_peer_item() -> impl Widget<TargetPeer> {
    Flex::row().with_child(
        Button::dynamic(|data: &String, _| data.clone())
            .lens(Identity.map(
                |d: &TargetPeer| format!("{}\n{}:{}", d.hostname, d.ip, d.port),
                |_, _| {},
            ))
            .on_click(|ctx: &mut EventCtx, data: &mut TargetPeer, _| {
                ctx.get_external_handle()
                    .submit_command(SET_CURRENT_PEER, data.clone(), Target::Auto)
                    .expect("command failed to submit");
            })
            .fix_width(180.0),
    )
}

fn main() {
    let args: Vec<String> = env::args().collect();

    let app_state = AppState::new();

    let window = WindowDesc::new(build_gui())
        .title(format!(
            "File Transfer @ {}",
            hostname::get().unwrap().into_string().unwrap()
        ))
        .window_size((600.0, 300.0));

    let launcher = AppLauncher::with_window(window);

    let event_sink = launcher.get_external_handle();
    let mut port: String = RANDOM_PORT.to_string();
    if args.len() > 1 && !args[1].is_empty() {
        port = args[1].clone();
    }

    let download_dir_rcvr = Arc::clone(&app_state.download_dir_rcvr);
    thread::spawn(move || {
        start_tokio(download_dir_rcvr, event_sink, &port);
    });

    launcher
        .delegate(Delegate)
        .log_to_console()
        .launch(app_state)
        .expect("Failed to launch application");
}

fn set_icon() -> TrayItem {
    let mut tray = TrayItem::new("File Transfer", IconSource::Resource("exe-icon")).unwrap();

    tray.add_label("File Transfer").unwrap();
    tray.add_menu_item("Hello", || {
        println!("Hello!");
    })
    .unwrap();

    tray.set_icon(IconSource::Resource("exe-icon")).unwrap();
    tray
}

#[tokio::main]
async fn start_tokio(download_dir: Arc<Receiver<String>>, sink: ExtEventSink, port: &str) {
    let listener = TcpListener::bind(format!("{}:{}", "0.0.0.0", port))
        .await
        .unwrap();

    let _tray = set_icon();
    let port = listener.local_addr().unwrap().port();
    let sink_clone = sink.clone();
    thread::spawn(move || {
        autodiscovery::start(sink_clone, port);
    });

    loop {
        let (socket, _) = listener.accept().await.unwrap();
        let sink_clone = sink.clone();
        let download_dir_rcvr_clone = download_dir.clone();
        let mut receiver = DataReceiver::new(download_dir_rcvr_clone, sink_clone, socket);
        if let Err(error) = receiver.process_incoming().await {
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
        }
    }
}
