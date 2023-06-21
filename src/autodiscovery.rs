use druid::{ExtEventSink, Selector, Target};
use local_ip_address::local_ip;
use searchlight::{
	broadcast::{BroadcasterBuilder, ServiceBuilder},
	net::{IpVersion, TargetInterfaceV4, TargetInterfaceV6}, discovery::{DiscoveryBuilder, DiscoveryEvent}, dns::{rr::Record, op::DnsResponse},
};
use std::{
	net::{IpAddr, Ipv4Addr},
	str,
	str::FromStr, sync::mpsc, time::Duration,
};

use crate::PeerAddress;

pub(crate) const HOST_ADDRESS_VAL_FN: Selector<String> = Selector::new("host_address_val_fn");
pub(crate) const HOST_PORT_VAL_FN: Selector<String> = Selector::new("host_port_val_fn");
const SERVICE_TYPE: &'static str = "_filesend._tcp.local";

pub(crate) fn start(sink: ExtEventSink, port: u16) {
    let (discovery_tx, discovery_rx) = mpsc::sync_channel(100);

	let local_ip = local_ip().unwrap().to_string();
    println!("This is my local IP address: {}", local_ip);

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

	std::thread::sleep(Duration::from_secs(1));
	let _discovery = DiscoveryBuilder::new()
		.loopback()
		.service(SERVICE_TYPE)
		.unwrap()
		.build(IpVersion::Both)
		.unwrap()
		.run_in_background(move |event| {
			if let DiscoveryEvent::ResponderFound(responder) = event {
				// println!("responder: {:?}", responder);
				match discovery_tx.try_send(responder) {
					Ok(_) => {},
					Err(e) => eprintln!("Got error: {:?}", e.to_string()),
				};
			}
		});

	loop {
		println!("Receiving events...");
		let event = discovery_rx.recv();
		println!("Got event");
		if event.is_ok() {
			let addr: Option<PeerAddress> = get_target_address(&event.unwrap().last_response, &local_ip);
			println!("addrs: {:?}", addr);

			match addr {
				Some(a) => {
					update_host_addr(&sink, a.ip);
					update_host_port(&sink, a.port.to_string());
				},			
				None => {},
			}
		} else {
			eprintln!("Error while receiving discovery event: {:?}", event.err());
		}
	}
}

fn get_target_address(dns_response: &DnsResponse, local_ip: &String) -> Option<PeerAddress> {
    let mut target_ip_v4: Option<String> = None;
    let mut target_ip_v6: Option<String> = None;
    let mut target_port: Option<u16> = None;
	dns_response.additionals().iter()
		.for_each(|record|     
			match record.data() {
				Some(searchlight::dns::rr::RData::TXT(_txt)) => {},
				Some(searchlight::dns::rr::RData::SRV(srv)) => {
					println!("srv: {:?}", srv);
					target_port = Some(srv.port());
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
				Some(&_) => println!("Other"),
				None => println!("None: {:?}", record),
			}
	);

	println!("ipv4: {:?}, ipv6: {:?}, port: {:?}", target_ip_v4, target_ip_v6, target_port);

    if (target_ip_v4.is_some() || target_ip_v6.is_some()) && target_port.is_some() {
        return Some(PeerAddress { ip: target_ip_v4.unwrap(), port: target_port.unwrap() });
    } else {
        return None;
    }
}

fn update_host_addr(sink: &ExtEventSink, value: String) {
    sink.submit_command(HOST_ADDRESS_VAL_FN, value, Target::Auto)
        .expect("command failed to submit");
}

fn update_host_port(sink: &ExtEventSink, value: String) {
    sink.submit_command(HOST_PORT_VAL_FN, value, Target::Auto)
        .expect("command failed to submit");
}
