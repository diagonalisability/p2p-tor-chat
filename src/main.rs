// main.rs
enum Message {
	PeerMessage(String/*sender*/,String/*body*/)
}
struct TorChild {
	process: std::process::Child,
	stdout_lines: std::io::Lines<std::io::BufReader<std::process::ChildStdout>>,
}
impl TorChild {
	fn new(profile_dir:&String,torrc:&String)->TorChild {
		println!("starting tor, logging to {}/log",profile_dir);
		let mut process= std::process::Command::new("tor")
			.args(&[
				"-f",torrc,
				"ControlPort", &format!("unix:{}/control-socket",profile_dir),
				"DisableNetwork", "1",
				"DataDirectory", "data",
				"SocksPort", "auto",//&format!("unix:{}/socks-socket",profile_dir),
//				"CookieAuthentication","1",
//				"CookieAuthFile","cookie",
				"Log","info file tor-log",
				"Log","notice",
			])
			.current_dir(profile_dir)
			.stdout(std::process::Stdio::piped())
			.spawn()
			.unwrap();
		use std::io::BufRead;
		let stdout= process.stdout.take().unwrap();
		TorChild {
			process: process,
			stdout_lines: std::io::BufReader::new(stdout).lines()
		}
	}
	fn wait_for_ready(&mut self) {
		use std::io::BufRead;
		loop {
			if self.stdout_lines.next().unwrap().unwrap().contains("Opened Control listener") {
				println!("control listener open");
				return;
			}
		}
	}
	// call this after enabling network
	fn get_socks_port(&mut self)->u32 {
		use std::io::BufRead;
		loop {
			let line= self.stdout_lines.next().unwrap().unwrap();
			if line.contains("Opened Socks listener on") {
				let port= line.rsplit(":").next().unwrap().parse().unwrap();
				println!("socks port: {}",port);
				return port;
			}
		}
	}
}
impl Drop for TorChild {
	fn drop(&mut self) {
		println!("dropping tor");
		std::process::Command::new("kill") // sends sigterm
			.args(&[&format!("{}",self.process.id())])
			.status()
			.unwrap();
	}
}
fn connect_to_peer(socks_port:u32) {
	use gtk::prelude::*;
	let glade_src= std::fs::read_to_string("peer-connect.glade").unwrap();
	let builder= gtk::Builder::from_string(&glade_src);
	let window:gtk::Window= builder.get_object("window0").unwrap();
	let button:gtk::Button= builder.get_object("button0").unwrap();
	let entry:gtk::Entry= builder.get_object("textfield0").unwrap();
	button.connect_clicked(move/*entry*/|_but| {
		println!("button clicked with entry text '{}'",entry.get_buffer().get_text());
		//connect_with_address(entry.get_buffer().get_text());
		let mut stream= socks::Socks5Stream::connect(
			format!("127.0.0.1:{}",socks_port),
			format!("{}:20001",entry.get_buffer().get_text().as_str()).as_str()
		).unwrap();
		println!("connected!");
		std::io::Write::write(&mut stream,b"asdfasdfasdf\nbob\nhello, there!\nquit\n");
		let reader= std::io::BufReader::new(stream);
		use std::io::BufRead;
		println!("waiting for response");
		println!("got response: {}",reader.lines().next().unwrap().unwrap());
		// https://docs.rs/reqwest/0.10.9/reqwest/struct.Proxy.html
		//let client= reqwest::Client::builder()
		//	.proxy(reqwest::Proxy::all(&format!("socks5://127.0.0.1:{}",SOCKS_PORT)).unwrap())
		//	.build().unwrap();
	});
	window.show_all();
}
// start gui
fn run_gtk(
	receiver:glib::Receiver<Message>,
	profile_dir_sender:tokio::sync::oneshot::Sender<String>,
	socks_port_receiver:tokio::sync::oneshot::Receiver<u32>
) {
	use gtk::prelude::*;
	gtk::init().unwrap();
	println!("gtk started");
	{
		let glade_src= std::fs::read_to_string("profile-select.glade").unwrap();
		let builder= gtk::Builder::from_string(&glade_src);
		let window:gtk::Window= builder.get_object("window0").unwrap();
		let button:gtk::Button= builder.get_object("button0").unwrap();
		let entry:gtk::Entry= builder.get_object("textfield0").unwrap();
		{
			// this is horrible but it works
			let profile_dir_sender= std::rc::Rc::new(std::cell::Cell::new(Some(profile_dir_sender)));
			let socks_port_receiver= std::rc::Rc::new(std::cell::Cell::new(Some(socks_port_receiver)));
			let window= window.clone();
			button.connect_clicked(move |_but| {
				profile_dir_sender.take().unwrap().send(entry.get_buffer().get_text()).unwrap();
				println!("profile selected, moving on with gui");
				window.close();
				let socks_port= tokio::runtime::Runtime::new().unwrap().block_on(async {
					socks_port_receiver.take().unwrap().await.unwrap()
				});
				connect_to_peer(socks_port);
			});
		}
		window.show_all();
	}
	receiver.attach(
		None/*use thread-default context*/,
		|msg:Message| {
			println!("got a message");
			match msg {
				Message::PeerMessage(sender,body)=> {
					println!("{} says: {}",sender,body);
				}
			}
			glib::Continue(true)
		}
	);
	gtk::main();
}
#[tokio::main] // check size
async fn main() {
	let (profile_dir,sender,gtk_thread,socks_port_sender)= {
		// https://doc.rust-lang.org/std/sync/struct.Condvar.html
		let (msg_sender,msg_receiver)= glib::MainContext::channel(glib::PRIORITY_DEFAULT);
		let (profile_dir_sender,profile_dir_receiver)= tokio::sync::oneshot::channel();
		let (socks_port_sender,socks_port_receiver)= tokio::sync::oneshot::channel();
		let gtk_thread= std::thread::spawn(move||{run_gtk(msg_receiver,profile_dir_sender,socks_port_receiver)});
		(
			profile_dir_receiver.await.unwrap(),
			msg_sender,
			gtk_thread,
			socks_port_sender
		)
	};
	// make profile_dir absolute so that tor can use it for a unix socket
	let cur_dir= std::env::current_dir().unwrap().into_os_string().into_string().unwrap();
	let profile_dir= format!("{}/{}",cur_dir,profile_dir);
	println!("profile_dir is: {}",profile_dir);
	// now that gtk is running, we can send events through the channel
	//sender.send(Message::PeerMessage("bob".to_string(),"hi, alice!".to_string())).unwrap();
	let key_path= format!("{}/key",profile_dir);
	let onion_key= match std::fs::File::open(&key_path) {
		Ok(file)=> {
			println!("using previously-generated key");
			let mut buf= [0u8;64];
			std::io::Read::read_exact(&mut file.try_clone().unwrap(),&mut buf).unwrap();
			torut::onion::TorSecretKeyV3::from(buf)
		},
		Err(ref e) if e.kind()==std::io::ErrorKind::NotFound=> {
			std::fs::DirBuilder::new()
				.recursive(true)
				.create(profile_dir.clone()).unwrap();
			let mut perms= std::fs::metadata(profile_dir.clone()).unwrap().permissions();
			std::os::unix::fs::PermissionsExt::set_mode(&mut perms,0o700);
			std::fs::set_permissions(profile_dir.clone(),perms).unwrap();
			let mut key_file= std::fs::File::create(&key_path).unwrap(); // fails if prof dir DNE
			println!("generating new key");
			let key= torut::onion::TorSecretKeyV3::generate();
			std::io::Write::write(&mut key_file,&key.as_bytes()).unwrap();
			// write key to profile for next time
			key
		},
		Err(_)=> panic!("error opening key file")
	};
	// https://github.com/teawithsand/torut/blob/master/examples/make_onion_v3.rs
	// https://github.com/teawithsand/torut/blob/master/examples/cookie_authenticate.rs
	// https://comit.network/blog/2020/07/02/tor-poc/
	println!("start tor");
	let mut tor_child= TorChild::new(&profile_dir,&format!("{}/torrc",cur_dir));
	let socks_port= tor_child.wait_for_ready();
	println!("connected to control port");
	let mut ucon;
	{
		use tokio::net::UnixStream;
		let unix= UnixStream::connect(format!("{}/control-socket",profile_dir)).await.unwrap();
		ucon= torut::control::UnauthenticatedConn::new(unix);
	}
	println!("created connection");
	let proto_info= ucon.load_protocol_info().await.unwrap();
//	assert!(proto_info.auth_methods.contains(&torut::control::TorAuthMethod::Cookie), "Null authentication is not allowed");
	let auth_data= proto_info.make_auth_data().unwrap().unwrap();
	ucon.authenticate(&auth_data).await.unwrap();
	let mut acon= ucon.into_authenticated().await;
	if false {
		// if you take out this statement, some super weird type inference
		// that this library authour was depending on fails to occur and the call to
		// into_authenticated fails because of an unknown type
		acon.set_async_event_handler(Some(|_| { async move { Ok(()) } }));
		//let _socksport = acon.get_info_unquote("net/listeners/socks").await.unwrap();
	}
	let listener= std::net::TcpListener::bind("127.0.0.1:0").unwrap();
	let port= listener.local_addr().unwrap().port();
	println!("the OS gave us port {}",port);
	acon.add_onion_v3(&onion_key,false,false,false,Some(0),&mut[
		(20001,std::net::SocketAddr::new(std::net::IpAddr::from(std::net::Ipv4Addr::new(127,0,0,1)),port))
	].iter()).await.unwrap();
	println!("using onion address {}",onion_key.public().get_onion_address());
	acon.set_conf("DisableNetwork",Some("0")).await.unwrap();

	println!("getting socks port");
	let socks_port= tor_child.get_socks_port();
	socks_port_sender.send(socks_port).unwrap();

	let (sender,receiver)= std::sync::mpsc::channel::<String>();

	std::thread::spawn(move||{ for stream in listener.incoming() {
		let sender= sender.clone();
		std::thread::spawn(move||{
			println!("got a tor connection!");
			let mut write_stream= stream.unwrap();
			let read_stream= write_stream.try_clone().unwrap();
			let mut reader= std::io::BufReader::new(&read_stream);
			let mut peer_id= String::new(); std::io::BufRead::read_line(&mut reader,&mut peer_id).unwrap(); let peer_id= peer_id.trim();
			println!("peer identifies themself as: {}",peer_id);
			println!("authenticating peer by sending cookie");
			// TODO (open a stream to peer_id.onion, peer should accept and send the cookie back down this stream)
			println!("peer authenticated");
			let mut nick= String::new(); std::io::BufRead::read_line(&mut reader,&mut nick).unwrap(); let nick= nick.trim();
			println!("peer nickname: {}",nick);
			let mut line= String::new();
			loop {
				line.clear();
				std::io::BufRead::read_line(&mut reader,&mut line).unwrap(); let line= line.trim();
				sender.send(format!("{} says: '{}'",nick,line)).unwrap();
				if line.trim()=="quit" {
					break;
				}
				std::io::Write::write(&mut write_stream,b"hello!\n").unwrap();
			}
		});
	}});
	gtk_thread.join().unwrap();
}
