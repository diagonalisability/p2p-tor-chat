// main.rs
#[derive(Clone)]
struct GrowingThreadPool {
	// idk if Mutex is overkill but it seems to do what i want
	o: std::sync::Arc<std::sync::Mutex<threadpool::ThreadPool>>,
}
impl GrowingThreadPool {
	fn new()->GrowingThreadPool {
		GrowingThreadPool {
			o: std::sync::Arc::new(std::sync::Mutex::new(threadpool::ThreadPool::new(4))),
		}
	}
	fn execute<F:FnOnce()+Send+'static>(&self,job:F) {
		let mut o= self.o.lock().unwrap();
		if 0<o.queued_count() {
			let new_count= o.max_count()+1;
			o.set_num_threads(new_count);
		}
		o.execute(job);
	}
	fn join(&self) {
		self.o.lock().unwrap().join();
	}
}
// join if this is the last reference (ideal for cloning and passing around)
impl Drop for GrowingThreadPool {
	fn drop(&mut self) {
		if std::sync::Arc::strong_count(&self.o)==1 {
			self.join();
		}
	}
}
struct TorChild {
	process: std::process::Child,
	acon: torut::control::AuthenticatedConn<
		tokio::net::UnixStream,
		fn(torut::control::AsyncEvent<'static>)->DummyFuture>,
	socks_port: u16,
}
#[derive(Copy,Clone)]
struct TorConnector {
	socks_port: u16
}
impl std::fmt::Debug for TorChild {
	fn fmt(&self,f:&mut std::fmt::Formatter<'_>)-> Result<(),std::fmt::Error> {
		f.write_str("TorChild{...}")
	}
}
// torut never needs this, but the AuthenticatedConn struct has a weird type parameter
// which this is used to fix.
struct DummyFuture {}
impl std::future::Future for DummyFuture {
	type Output= Result<(),torut::control::ConnError>;
	fn poll(self:std::pin::Pin<&mut Self>,_cx:&mut std::task::Context<'_>)->std::task::Poll<Self::Output> {
		panic!("this way i don't need to return anything");
	}
}
impl TorChild {
	async fn new(profile_dir:&String,torrc:&String)->TorChild {
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
				"Log","notice file tor-log",
				"Log","notice",
			])
			.current_dir(profile_dir)
			.stdout(std::process::Stdio::piped())
			.spawn()
			.unwrap();
		use std::io::BufRead;
		let stdout= process.stdout.take().unwrap();
		let mut lines= std::io::BufReader::new(stdout).lines();
		// wait for tor become ready to accept control connections
		loop {
			if lines.next().unwrap().unwrap().contains("Opened Control listener") {
				println!("control listener open");
				break;
			}
		}
		let mut ucon;
		{
			use tokio::net::UnixStream;
			let unix= UnixStream::connect(format!("{}/control-socket",profile_dir)).await.unwrap();
			ucon= torut::control::UnauthenticatedConn::new(unix);
		}
		println!("created connection");
		let proto_info= ucon.load_protocol_info().await.unwrap();
		let auth_data= proto_info.make_auth_data().unwrap().unwrap();
		ucon.authenticate(&auth_data).await.unwrap();
		let mut acon= ucon.into_authenticated().await;
		acon.take_ownership().await.unwrap();
		// get socks port
		acon.set_conf("DisableNetwork",Some("0")).await.unwrap();
		println!("getting socks port");
		let socks_port= loop {
			let line= lines.next().unwrap().unwrap();
			if line.contains("Opened Socks listener on") {
				break line.rsplit(":").next().unwrap().parse().unwrap();
			}
		};
		println!("socks port: {}",socks_port);
		TorChild {
			process: process,
			acon: acon,
			socks_port: socks_port,
		}
	}
	fn make_connector(&self)->TorConnector {
		TorConnector {
			socks_port: self.socks_port
		}
	}
	async fn make_onion_listener(
		&mut self,
		key:&torut::onion::TorSecretKeyV3
	)->std::net::TcpListener {
		let listener= std::net::TcpListener::bind("127.0.0.1:0").unwrap();
		let port= listener.local_addr().unwrap().port();
		println!("the OS gave us port {}",port);
		self.acon.add_onion_v3(&key,false,false,false,Some(0),&mut[
			(VIRTUAL_PORT,std::net::SocketAddr::new(std::net::IpAddr::from(std::net::Ipv4Addr::new(127,0,0,1)),port))
		].iter()).await.unwrap();
		println!("using onion address {}",key.public().get_onion_address());
		listener
	}
}
impl TorConnector {
	fn connect(self,address:&str,port:u16)->std::net::TcpStream {
		socks::Socks5Stream::connect(
			format!("127.0.0.1:{}",self.socks_port),
			format!("{}:{}",address,port).as_str(),
		).unwrap().into_inner()
	}
}
impl Drop for TorChild {
	fn drop(&mut self) {
		// this might not be necessary because we take ownership of the control stream
		println!("dropping tor");
		std::process::Command::new("kill") // sends sigterm
			.args(&[&format!("{}",self.process.id())])
			.status()
			.unwrap();
	}
}
fn make_gtk_label(text:&String)->gtk::Label {
	use gtk::prelude::*;
	let label= gtk::Label::new(Some(text));
	label.set_halign(gtk::Align::Start);
	label
}
fn display_image(messages:&gtk::ListBox,filename:&str) {
	use gtk::prelude::*;
	const MAX_IMAGE_SIDELENGTH:i32= 100;
	let image= gdk_pixbuf::Pixbuf::from_file(filename).unwrap();
	let (owidth,oheight)= (image.get_width(),image.get_height()); // original
	use std::cmp::min;
	let width0= min(owidth,MAX_IMAGE_SIDELENGTH);
	let height0= width0 * oheight / owidth;
	let height1= min(oheight,MAX_IMAGE_SIDELENGTH);
	let width1= height1 * owidth / oheight;
	let (width,height)= (min(width0,width1),min(height0,height1));
	let image= image.scale_simple(
		width,
		height,
		gdk_pixbuf::InterpType::Bilinear
	).unwrap();
	let image= gtk::Image::from_pixbuf(Some(&image));
	messages.add(&image);
}
#[derive(serde::Serialize,serde::Deserialize)]
enum Message {
	Text(String),
	FileOffer {
		file_id: u64,
		file_size: u64
	},
	FileRequest(u64), // file_id
}
// called from non-gtk thread
fn open_chat(
	peer_public_key:torut::onion::TorPublicKeyV3,
	streams:[std::net::TcpStream;2],
	profile_name:std::sync::Arc<String>,
	thread_pool:GrowingThreadPool
) {
	println!("opening chat");
	let streams= std::sync::Arc::new(std::sync::Mutex::new(Some(streams)));
	glib::source::idle_add(move||{
		let streams= streams.lock().unwrap().take().unwrap();
		let read_stream= streams[0].try_clone().unwrap();
		let write_stream= std::sync::Arc::new(std::sync::Mutex::new(read_stream.try_clone().unwrap()));
		let builder= open_glade("chat.glade");
		use gtk::prelude::*;
		let window:gtk::Window= builder.get_object("window").unwrap();
		let messages:gtk::ListBox= builder.get_object("messages").unwrap();
		let entry:gtk::Entry= builder.get_object("entry").unwrap();
		messages.add(&make_gtk_label(&format!("chat for profile: {}",profile_name)));
		{
			let messages= messages.clone();
			entry.connect_activate(move|entry|{
				let buf= entry.get_buffer();
				let message= buf.get_text();
				buf.set_text("");
				let label= make_gtk_label(&format!("you: {}",message));
				label.show();
				messages.add(&label);
				use std::io::Write;
				println!("sending message: '{}'",message);
				let mut write_stream= write_stream.lock().unwrap();
				write_stream.write_all(&bincode::serialize(&Message::Text(message)).unwrap()).unwrap();
			});
		}
		window.show_all();
		let (tx,rx)= glib::MainContext::channel(glib::PRIORITY_DEFAULT);
		rx.attach(
			None, // use default context
			move|msg| {
				println!("got message on the main thread: {}",msg);
				let text= format!("peer: {}",msg);
				let label= gtk::Label::new(Some(&text));
				label.set_halign(gtk::Align::Start);
				println!("adding label with text {}",text);
				messages.add(&label);
				label.show();
				glib::Continue(true)
			}
		);
		thread_pool.execute(move||{
			let tx= tx.clone();
			loop {
				let message:Message=
					bincode::deserialize_from(read_stream.try_clone().unwrap()).unwrap();
				println!("parsed the message!");
				use Message::*;
				match message {
					Text(str)=> {
						println!("peer sent text: '{}'",str);
						tx.send(str).unwrap();
					}
					_=> { todo!("got some other kind of message"); }
				}
			}
		});
		let chooser= gtk::FileChooserDialog::with_buttons::<gtk::Window>(
			None, // title
			None, // parent
			gtk::FileChooserAction::Open,
			&[("send",gtk::ResponseType::Accept)],
		);
		let image_button:gtk::Button= builder.get_object("send-file-button").unwrap();
		image_button.connect_clicked(move|_button| {
			println!("image button clicked!");
			let chooser_c= chooser.clone();
			chooser.connect_response(move|_dialog,_response_type| {
				println!(
					"the selected file was {:?}",
					gio::FileExt::get_path(&chooser_c.get_file().unwrap()).unwrap()
				);
				chooser_c.hide();
			});
			chooser.run();
		});
		glib::Continue(false)
	});
}

fn connect_to_peer(
	tor_connector:TorConnector,
	my_public_key:torut::onion::TorPublicKeyV3,
	peer_address:&str,
	nonce_peer_map:&std::sync::Arc<std::sync::Mutex<std::collections::BTreeMap<Nonce,Peer>>>,
) {
	println!("generating nonce");
	let mut nonce= [0u8;NONCE_SIZE];
	use rand::RngCore;
	rand::rngs::OsRng.fill_bytes(&mut nonce);
	println!(
		"actual nonce is {}...{}",
		nonce[0],
		nonce[NONCE_SIZE-1],
	);
	let torut_peer_address:torut::onion::OnionAddressV3=
		std::str::FromStr::from_str(peer_address.strip_suffix(".onion").unwrap()).unwrap();
	let peer_public_key= torut_peer_address.get_public_key();
	{
		let mut nonce_peer_map= nonce_peer_map.lock().unwrap();
		nonce_peer_map.insert(nonce,Peer {
			public_key:peer_public_key,
			streams:[None,None],
		});
	}
	println!("connecting to {}",peer_address);
	let mut stream= tor_connector.connect(peer_address,VIRTUAL_PORT);
	println!("connected!");
	let stream_type= [2];
	use std::io::Write;
	stream.write(&stream_type).unwrap();
	stream.write(&my_public_key.to_bytes()).unwrap();
	stream.write(&nonce).unwrap();
	// the receiver's job is to "call me back", so drop this connection now
}
const NONCE_SIZE:usize= 256; // 256 bytes * 8 bits/byte = 2048 bits
type Nonce= [u8;NONCE_SIZE];
const VIRTUAL_PORT:u16= 20001;
fn prompt_for_peer_address(
	tor_connector:TorConnector,
	thread_pool:GrowingThreadPool,
	my_public_key:torut::onion::TorPublicKeyV3,
	nonce_peer_map:std::sync::Arc<std::sync::Mutex<std::collections::BTreeMap<Nonce,Peer>>>,
) {
	use gtk::prelude::*;
	let builder= open_glade("peer-connect.glade");
	let window:gtk::Window= builder.get_object("window0").unwrap();
	let button:gtk::Button= builder.get_object("button0").unwrap();
	let entry:gtk::Entry= builder.get_object("textfield0").unwrap();
	let window_c= window.clone();
	button.connect_clicked(move|_| {
		let entry_text= entry.get_buffer().get_text();
		println!("button clicked with entry text '{}'",entry_text);
		let nonce_peer_map= nonce_peer_map.clone();
		thread_pool.execute(move||{connect_to_peer(
			tor_connector,
			my_public_key,
			&entry_text,
			&nonce_peer_map,
		)});
		// TODO make sure the address parses correctly
		window_c.close();
	});
	window.show_all();
}
fn open_glade(path:&str)->gtk::Builder {
	let src= std::fs::read_to_string(path).unwrap();
	gtk::Builder::from_string(&src)
}
fn select_profile<CB:Fn(String)+'static>(cb:CB) {
	use gtk::prelude::*;
	let builder= open_glade("profile-select.glade");
	let window:gtk::Window= builder.get_object("window0").unwrap();
	let button:gtk::Button= builder.get_object("button0").unwrap();
	let entry:gtk::Entry= builder.get_object("textfield0").unwrap();
	{
		let window= window.clone();
		button.connect_clicked(move|_button| {
			println!("profile selected, moving on with gui");
			window.close();
			cb(entry.get_buffer().get_text());
		});
	}
	window.show_all();
}
struct Peer {
	public_key:torut::onion::TorPublicKeyV3,
	streams:[Option<std::net::TcpStream>;2],
}
fn listen(
	listener:std::net::TcpListener,
	thread_pool:GrowingThreadPool,
	profile_name:std::sync::Arc<String>,
	tor_connector:TorConnector,
	nonce_peer_map:std::sync::Arc<std::sync::Mutex<std::collections::BTreeMap<Nonce,Peer>>>,
) {
	use std::io::{Read,Write};
	for stream in listener.incoming() {
		println!("got listen stream, queueing handle task");
		let profile_name= profile_name.clone();
		// https://docs.rs/rand/0.8.0/rand/rngs/struct.OsRng.html
		let thread_pool_c= thread_pool.clone();
		let nonce_peer_map= nonce_peer_map.clone();
		thread_pool.execute(move||{
			let mut stream= stream.unwrap();
			let mut stream_type= [0u8];
			stream.read_exact(&mut stream_type).unwrap();
			let stream_type= stream_type[0];
			match stream_type {
				// initial connection
				2=> {
					let mut peer_public_key= [0u8;32];
					let mut nonce= [0u8;NONCE_SIZE];
					stream.read_exact(&mut peer_public_key).unwrap();
					stream.read_exact(&mut nonce).unwrap();
					println!(
						"received nonce {}...{}",
						nonce[0],
						nonce[NONCE_SIZE-1],
					);
					// there should be some validation here
					let peer_public_key= torut::onion::TorPublicKeyV3::from_bytes(&peer_public_key).unwrap();
					let peer_address= peer_public_key.get_onion_address();
					// streams[0] is the blocking stream, streams[1] is the nonblocking stream
					let streams:[std::net::TcpStream;2]= std::convert::TryFrom::try_from((0..2).map(|stream_type:u8| {
						let mut stream= tor_connector.connect(&format!("{}",peer_address),VIRTUAL_PORT);
						// connecting back to the originator using their nonce
						let stream_type= [stream_type];
						stream.write(&stream_type).unwrap();
						stream.write(&nonce).unwrap();
						stream
					}).collect::<Vec<_>>()).unwrap();
					open_chat(
						peer_public_key,
						streams,
						profile_name,
						thread_pool_c,
					);
				}
				0|1=> {
					let mut nonce= [0u8;NONCE_SIZE];
					stream.read_exact(&mut nonce).unwrap();
					let nonce_peer_map= nonce_peer_map.clone();
					let mut nonce_peer_map= nonce_peer_map.lock().unwrap();
					let peer= nonce_peer_map.get_mut(&nonce).unwrap();
					let stream_type:usize= stream_type.into();
					peer.streams[stream_type].replace(stream);
					if peer.streams.iter().all(Option::is_some) {
						println!("both streams received, sending peer to main thread for chat gui");
						let profile_name= profile_name.clone();
						open_chat(
							peer.public_key,
							std::convert::TryFrom::try_from(peer.streams.iter().map(|x|x.as_ref().unwrap().try_clone().unwrap()).collect::<Vec<_>>()).unwrap(),
							profile_name,
							thread_pool_c,
						);
					}
					println!("one stream received, waiting for the other...");
				}
				_ => panic!("bad stream_type sent from peer")
			}
		});
	}
}
fn open_menu(
	profile_name:String,
	thread_pool:GrowingThreadPool,
	tor_child_rc:std::rc::Rc<std::cell::Cell<Option<TorChild>>>,
) {
	let profile_name= std::sync::Arc::new(profile_name);
	use gtk::prelude::*;
	// make profile_dir absolute so that tor can use it for a unix socket
	let cur_dir= std::env::current_dir().unwrap().into_os_string().into_string().unwrap();
	let profile_dir= format!("{}/{}",cur_dir,profile_name);
	println!("profile_dir is: {}",profile_dir);
	let onion_key= get_onion_key(&profile_dir);
	println!("start tor");
	let rt= tokio::runtime::Runtime::new().unwrap();
	let nonce_peer_map= std::sync::Arc::new(std::sync::Mutex::new(std::collections::BTreeMap::new()));
	let tor_child= rt.block_on(async {
		let mut tor_child= TorChild::new(&profile_dir,&format!("{}/torrc",cur_dir)).await;
		println!("connected to control port");
		let listener= tor_child.make_onion_listener(&onion_key).await;
		let thread_pool_c= thread_pool.clone();
		let tor_connector= tor_child.make_connector();
		let nonce_peer_map= nonce_peer_map.clone();
		thread_pool.execute(move||{listen(
			listener,
			thread_pool_c,
			profile_name,
			tor_connector,
			nonce_peer_map,
		)});
		tor_child
	});
	let tor_connector= tor_child.make_connector();
	tor_child_rc.set(Some(tor_child));
	let builder= open_glade("menu.glade");
	let button:gtk::Button= builder.get_object("connect_button").unwrap();
	let window:gtk::Window= builder.get_object("window").unwrap();
	window.resize(300,100);
	let thread_pool= std::rc::Rc::new(std::cell::RefCell::new(thread_pool));
	button.connect_clicked(move|_but| {
		let nonce_peer_map= nonce_peer_map.clone();
		prompt_for_peer_address(
			tor_connector,
			thread_pool.borrow().clone(),
			onion_key.public(),
			nonce_peer_map,
		);
	});
	window.show_all();
}
fn main() {
	let args:Vec<String>= std::env::args().collect();
	if args.len() > 2 {
		println!("arguments: profile_name");
	}
	println!("command-line arguments: {:?}",args);
	let thread_pool= GrowingThreadPool::new();
	gtk::init().unwrap();
	println!("gtk started");
	let thread_pool_rc= std::rc::Rc::new(std::cell::Cell::new(Some(thread_pool.clone())));
	let tor_child_rc= std::rc::Rc::new(std::cell::Cell::new(None));
	{
		// running the program with a profile name argument skips the dialog
		let tor_child_rc= tor_child_rc.clone();
		let open_menu= move|prof|{open_menu(
			prof,
			(*thread_pool_rc).take().unwrap(),
			tor_child_rc.clone()
		);};
		if args.len() == 2 {
			open_menu(args[1].clone());
		} else {
			select_profile(open_menu);
		}
	}
	gtk::main();
	thread_pool.join();
	drop(tor_child_rc);
	// tor_child_rc destructs here, killing tor
}
fn get_onion_key(profile_dir:&String)->torut::onion::TorSecretKeyV3 {
	let key_path= format!("{}/key",profile_dir);
	match std::fs::File::open(&key_path) {
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
	}
}
