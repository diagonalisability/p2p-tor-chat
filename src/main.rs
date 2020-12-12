// main.rs
enum Message {
	PeerMessage(String/*sender*/,String/*body*/)
}
const SOCKS_PORT:u32= 30001;
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
	lock:std::sync::Weak<std::sync::Mutex<std::option::Option<String>>>,
	cv:std::sync::Arc<std::sync::Condvar>
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
			let window_c= window.clone();
			button.connect_clicked(move/*entry*/|_but| {
				*(*lock.upgrade().unwrap()).lock().unwrap()= Some(entry.get_buffer().get_text());
				window_c.close();
				(*cv).notify_one(); // notify the original thread
				println!("profile selected, moving on with gui");
				connect_to_peer();
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
		use std::sync::{Arc,Mutex,Condvar};
		use std::option::Option;
		// https://doc.rust-lang.org/std/sync/struct.Condvar.html
		let (sender,receiver)= glib::MainContext::channel(glib::PRIORITY_DEFAULT);
		let lock= Arc::new(Mutex::new(Option::<String>::None));
		let cv= Arc::new(Condvar::new());
		let gtk_thread= {
			let lock_c= Arc::<Mutex<Option<std::string::String>>>::downgrade(&lock);
			let cv_c= cv.clone();
			std::thread::spawn(move||{run_gtk(receiver,lock_c,cv_c)})
		};
		{
			let mut guard= (*lock).lock().unwrap();
			while !(1==Arc::<Mutex<Option<String>>>::strong_count(&lock) && (*guard).is_some()) {
				guard= (*cv).wait(guard).unwrap();
			};
		}
		(
			Arc::<Mutex<Option<String>>>::try_unwrap(lock).unwrap().into_inner().unwrap().unwrap(),
			sender,
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
	let tor_pid= std::process::Command::new("tor")
		.args(&[
			"-f","torrc",
			"ControlPort", "10001",
			"DisableNetwork", "1",
			"SocksPort", &format!("{}",SOCKS_PORT),
			"CookieAuthentication","1",
			"CookieAuthFile","cookie",
//			"Log","debug",
		])
		.current_dir("tor")
		.spawn()
		.unwrap()
		.id();
	// give tor time to start, otherwise we won't be able to connect (this is a hack)
	std::thread::sleep(std::time::Duration::from_millis(1000));
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

//	do_webview(receiver);
	gtk_thread.join().unwrap();
	println!("kill tor");
	std::process::Command::new("kill")
		.args(&[&format!("{}",tor_pid)])
		.status()
		.unwrap();
}
/*
fn do_webview(msg_receiver:std::sync::mpsc::Receiver<String>) {
	let html= "<html>
		<body>
			<button onclick=external.invoke('signal')>hi!</button>
		</body>
	</html>";
	let mut wv= web_view::builder()
		.content(web_view::Content::Html(html))
		.user_data(())
		.size(40,40)
		.invoke_handler(|wv, arg| {
			println!("got arg: {}",arg);
			wv.eval("
				{
					const div= document.createElement('div');
					div.innerHTML= 'hello hello hello hello hello hello hello hello hello hello hello hello hello hello hello hello hello hello hello hello hello hello hello hello hello hello hello hello hello hello hello hello hello hello hello hello hello hello hello hello hello hello hello';
					div.classList.add('message');
					document.body.appendChild(div);
				}
			").unwrap();
			Ok(())
		})
		.build()
		.unwrap();
	wv.inject_css("
		button {
			background-color: #73AD21;
			padding: 2mm;
			color: #ffffff;
			border: 0;
			border-radius:1mm;
		}
		body > * {		
			display: block;
		}
		.message {
			border-radius:1mm;
			margin: 4mm;
			padding: 1mm;
			background-color: #ffff00;
		}
	").unwrap();
	//std::thread::sleep(std::time::Duration::from_millis(10000000));
	//return;
	wv.run();
//	while !wv.step().is_none() {}
	println!("we're done here");
	loop {
		let msg= msg_receiver.recv().unwrap();
		println!("got message!: {}",msg);
	}
}
*/
