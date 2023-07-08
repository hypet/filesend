use druid::{ExtEventSink, Selector, Target};
use local_ip_address::local_ip;
use searchlight::{
	broadcast::{BroadcasterBuilder, ServiceBuilder},
	net::{IpVersion, TargetInterfaceV4, TargetInterfaceV6}, discovery::{DiscoveryBuilder, DiscoveryEvent}, dns::{op::DnsResponse},
};
use std::{
	net::{IpAddr, Ipv4Addr},
	str,
	str::FromStr, time::Duration,
};

use crate::TargetPeer;

pub(crate) const TARGET_PEER_ADD_VAL_FN: Selector<TargetPeer> = Selector::new("target_peer_add_val_fn");
pub(crate) const TARGET_PEER_REMOVE_VAL_FN: Selector<TargetPeer> = Selector::new("target_peer_remove_val_fn");
const SERVICE_TYPE: &str = "_filesend._tcp";

pub(crate) fn start(sink: ExtEventSink, port: u16) {

	let local_ip = local_ip().unwrap().to_string();
    println!("Local address: {}:{}", local_ip, port);

	let _broadcaster = BroadcasterBuilder::new()
		.interface_v4(TargetInterfaceV4::All)
		.interface_v6(TargetInterfaceV6::All)
		.add_service(
			ServiceBuilder::new(SERVICE_TYPE, hostname::get().unwrap().into_string().unwrap(), port)
			.unwrap()
			.add_ip_address(IpAddr::V4(Ipv4Addr::from_str(local_ip.as_str()).unwrap()))
				.build()
				.unwrap(),
		)
		.build(IpVersion::Both)
		.unwrap()
		.run_in_background();

	std::thread::sleep(Duration::from_millis(500)); // Postpone due to GUI initialization
	let _discovery = DiscoveryBuilder::new()
		.loopback()
		.service(SERVICE_TYPE)
		.unwrap()
		.build(IpVersion::Both)
		.unwrap()
		.run_in_background(move |event| {
			match event {
				DiscoveryEvent::ResponderFound(responder) => {
					let addr: Option<TargetPeer> = get_target_address(&responder.last_response, &local_ip);
					if let Some(a) = addr {
						println!("New remote address: {:?}", a);
						update_target_peer(&sink, a);
					}
				}
				DiscoveryEvent::ResponderLost(responder) => {
					let addr: Option<TargetPeer> = get_target_address(&responder.last_response, &local_ip);
					if let Some(a) = addr {
						println!("Removing address: {:?}", a);
						remove_target_peer(&sink, a);
					}
				}
				DiscoveryEvent::ResponseUpdate { old: _responder_old, new: responder_new } => {
					let addr: Option<TargetPeer> = get_target_address(&responder_new.last_response, &local_ip);
					if let Some(a) = addr {
						println!("Update address: {:?}", a);
						update_target_peer(&sink, a);
					}
				}
			}
		});

	loop {
		std::thread::sleep(Duration::from_millis(100));
	}
}

fn get_target_address(dns_response: &DnsResponse, local_ip: &String) -> Option<TargetPeer> {
    let mut target_host: Option<String> = None;
    let mut target_ip_v4: Option<String> = None;
    let mut target_ip_v6: Option<String> = None;
    let mut target_port: Option<u16> = None;
	dns_response.additionals().iter()
		.for_each(|record|     {
			match record.data() {
				Some(searchlight::dns::rr::RData::TXT(_txt)) => {},
				Some(searchlight::dns::rr::RData::SRV(srv)) => {
					target_port = Some(srv.port());
					target_host = Some(srv.target().to_string());
				},
				Some(searchlight::dns::rr::RData::A(a_record)) => { 
					let a_rec_str = a_record.to_string();
					if a_rec_str.ne(&local_ip.to_string()) {
						target_ip_v4 = Some(a_rec_str);
					}
				},
				Some(searchlight::dns::rr::RData::AAAA(aaaa_record)) => {
					target_ip_v6 = Some(aaaa_record.to_string())
				},
				Some(&_) => {},
				None => {},
			}
		}
	);

    if (target_ip_v4.is_some() || target_ip_v6.is_some()) && target_port.is_some() {
        return Some(TargetPeer { 
			hostname: target_host.unwrap().replace(".local", "").replace(".", ""),  //TODO: get rid of .replace()
			ip: if target_ip_v4.is_some() { target_ip_v4.unwrap()} else { target_ip_v6.unwrap() }, 
			port: target_port.unwrap() 
		});
    } else {
        return None;
    }
}

fn update_target_peer(sink: &ExtEventSink, value: TargetPeer) {
    sink.submit_command(TARGET_PEER_ADD_VAL_FN, value, Target::Auto)
        .expect("command failed to submit");
}

fn remove_target_peer(sink: &ExtEventSink, value: TargetPeer) {
    sink.submit_command(TARGET_PEER_REMOVE_VAL_FN, value, Target::Auto)
        .expect("command failed to submit");
}
