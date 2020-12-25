// main.rs
enum Message {
	PeerMessage(String/*sender*/,String/*body*/)
}
const SOCKS_PORT:u32= 30001;
struct TorChild {
	process: std::process::Child
}
impl TorChild {
	fn new()->TorChild { println!("starting tor"); TorChild { process:
		std::process::Command::new("tor")
			.args(&[
				"-f","torrc",
				"ControlPort", "10001",
				"DisableNetwork", "1",
				"SocksPort", &SOCKS_PORT.to_string(),
				"CookieAuthentication","1",
				"CookieAuthFile","cookie",
				"Log","info file log",
			])
			.stdout(std::process::Stdio::piped())
			.current_dir("tor")
			.spawn()
			.unwrap()
	}}
	fn wait_for_ready(&mut self) {
		use std::io::BufRead;
		let stdout= self.process.stdout.take().unwrap();
		for line in std::io::BufReader::new(stdout).lines() {
			let line= line.unwrap();
			println!("{}",line);
			if line.contains("Opened Control listener") {
				println!("tor is now ready");
				return;
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
fn connect_to_peer() {
	use gtk::prelude::*;
	let glade_src= std::fs::read_to_string("peer-connect.glade").unwrap();
	let builder= gtk::Builder::from_string(&glade_src);
	let window:gtk::Window= builder.get_object("window0").unwrap();
	let button:gtk::Button= builder.get_object("button0").unwrap();
	let entry:gtk::Entry= builder.get_object("textfield0").unwrap();
	button.connect_clicked(move/*entry*/|_but| {
		println!("button clicked with entry text '{}'",entry.get_buffer().get_text());
		// https://docs.rs/reqwest/0.10.9/reqwest/struct.Proxy.html
		let proxy= reqwest::Proxy::http(&format!("socks5://127.0.0.1:{}",SOCKS_PORT));
	});
	window.show_all();
}
fn run_gtk(
	receiver:glib::Receiver<Message>,
	profile_dir_sender:tokio::sync::oneshot::Sender<String> 
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
			let window= window.clone();
			button.connect_clicked(move |_but| {
				profile_dir_sender.take().unwrap().send(entry.get_buffer().get_text()).unwrap();
				println!("profile selected, moving on with gui");
				connect_to_peer();
				window.close();
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
	let (profile_dir,sender,gtk_thread)= {
		// https://doc.rust-lang.org/std/sync/struct.Condvar.html
		let (msg_sender,msg_receiver)= glib::MainContext::channel(glib::PRIORITY_DEFAULT);
		let (profile_dir_sender,profile_dir_receiver)= tokio::sync::oneshot::channel();
		let gtk_thread= std::thread::spawn(move||{run_gtk(msg_receiver,profile_dir_sender)});
		(
			profile_dir_receiver.await.unwrap(),
			msg_sender,
			gtk_thread
		)
	};
	// now that gtk is running, we can send events through the channel
	sender.send(Message::PeerMessage("bob".to_string(),"hi, alice!".to_string())).unwrap();
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
				.create(profile_dir).unwrap();
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

	let mut tor_child= TorChild::new();
	tor_child.wait_for_ready();
	println!("tor started");
	let socket= tokio::net::TcpStream::connect("127.0.0.1:10001").await.unwrap();
	println!("connected to control port");
	let mut ucon= torut::control::UnauthenticatedConn::new(socket);
	println!("created connection");
	let proto_info= ucon.load_protocol_info().await.unwrap();
	assert!(proto_info.auth_methods.contains(&torut::control::TorAuthMethod::Cookie), "Null authentication is not allowed");
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
	acon.add_onion_v3(&onion_key,false,false,false,Some(0),&mut[
		(20001,std::net::SocketAddr::new(std::net::IpAddr::from(std::net::Ipv4Addr::new(127,0,0,1)),20002))
	].iter()).await.unwrap();
	println!("using onion address {}",onion_key.public().get_onion_address());
	acon.set_conf("DisableNetwork",Some("0")).await.unwrap();
	let listener= std::net::TcpListener::bind("127.0.0.1:20002").unwrap();

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
