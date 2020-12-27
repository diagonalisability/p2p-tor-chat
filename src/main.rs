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
	stdout_lines: std::io::Lines<std::io::BufReader<std::process::ChildStdout>>,
	acon: torut::control::AuthenticatedConn<
		tokio::net::UnixStream,
		fn(torut::control::AsyncEvent<'static>)->DummyFuture>
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
		Self::wait_for_ready(&mut lines);
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
		TorChild {
			process: process,
			stdout_lines: lines,
			acon: acon,
		}
	}
	fn wait_for_ready(lines:&mut std::io::Lines<std::io::BufReader<std::process::ChildStdout>>) {
		loop {
			if lines.next().unwrap().unwrap().contains("Opened Control listener") {
				println!("control listener open");
				return;
			}
		}
	}
	async fn make_onion_listener(&mut self,key:torut::onion::TorSecretKeyV3)->std::net::TcpListener {
		let listener= std::net::TcpListener::bind("127.0.0.1:0").unwrap();
		let port= listener.local_addr().unwrap().port();
		println!("the OS gave us port {}",port);
		self.acon.add_onion_v3(&key,false,false,false,Some(0),&mut[
			(20001,std::net::SocketAddr::new(std::net::IpAddr::from(std::net::Ipv4Addr::new(127,0,0,1)),port))
		].iter()).await.unwrap();
		println!("using onion address {}",key.public().get_onion_address());
		listener
	}
	// returns socks port
	async fn enable_network(&mut self)->u32 {
		self.acon.set_conf("DisableNetwork",Some("0")).await.unwrap();
		println!("getting socks port");
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
// called from non-gtk thread
fn open_chat(stream:std::net::TcpStream) {
	let stream= std::sync::Arc::new(std::sync::Mutex::new(Some(stream)));
	glib::source::idle_add(move||{
		println!("opening chat");
		let stream= stream.lock().unwrap().take().unwrap();
		let builder= open_glade("chat.glade");
		use gtk::prelude::*;
		let window:gtk::Window= builder.get_object("window").unwrap();
		window.show_all();
		glib::Continue(false)
	});
}
fn connect_to_peer(socks_port:u32,thread_pool:GrowingThreadPool) {
	use gtk::prelude::*;
	let builder= open_glade("peer-connect.glade");
	let window:gtk::Window= builder.get_object("window0").unwrap();
	let button:gtk::Button= builder.get_object("button0").unwrap();
	let entry:gtk::Entry= builder.get_object("textfield0").unwrap();
	let thread_pool= std::rc::Rc::new(std::cell::Cell::new(Some(thread_pool)));
	let window_c= window.clone();
	button.connect_clicked(move|_but| {
		let entry_text= entry.get_buffer().get_text();
	//	let entry_text= std::rc::Rc::new(std::cell::Cell::new(entry.get_buffer().get_text()));
		println!("button clicked with entry text '{}'",entry_text);
		window_c.close();
//		open_chat();
		thread_pool.take().unwrap().execute(move||{
			//connect_with_address(entry.get_buffer().get_text());
			println!("connecting to {}",entry_text);
			let mut stream= socks::Socks5Stream::connect(
				format!("127.0.0.1:{}",socks_port),
				format!("{}:20001",entry_text).as_str(),
			).unwrap().into_inner();
			println!("connected!");
			std::io::Write::write(&mut stream,b"asdfasdfasdf\nbob\n").unwrap();
			open_chat(stream);
			// https://docs.rs/reqwest/0.10.9/reqwest/struct.Proxy.html
			//let client= reqwest::Client::builder()
			//	.proxy(reqwest::Proxy::all(&format!("socks5://127.0.0.1:{}",SOCKS_PORT)).unwrap())
			//	.build().unwrap();
		});
	});
	window.show_all();
}
fn open_glade(path:&str)->gtk::Builder {
	let src= std::fs::read_to_string(path).unwrap();
	gtk::Builder::from_string(&src)
}
fn select_profile<CB:Fn(&String)+'static>(cb:CB) {
	use gtk::prelude::*;
	let builder= open_glade("profile-select.glade");
	let window:gtk::Window= builder.get_object("window0").unwrap();
	let button:gtk::Button= builder.get_object("button0").unwrap();
	let entry:gtk::Entry= builder.get_object("textfield0").unwrap();
	{
		let window= window.clone();
		button.connect_clicked(move |_but| {
//			profile_dir_sender.take().unwrap().send(entry.get_buffer().get_text()).unwrap();
			println!("profile selected, moving on with gui");
			window.close();
			cb(&entry.get_buffer().get_text());
//			let socks_port= tokio::runtime::Runtime::new().unwrap().block_on(async {
//				socks_port_receiver.take().unwrap().await.unwrap()
//			});
//			connect_to_peer(socks_port,thread_pool.take().unwrap());
		});
	}
	window.show_all();
}
fn listen(listener:std::net::TcpListener,thread_pool:GrowingThreadPool) {
	let thread_pool_c= thread_pool.clone();
	thread_pool.execute(move||{ for stream in listener.incoming() {
		println!("got listen stream, queueing handle task");
		thread_pool_c.execute(||{
			println!("got a tor connection!");
			let gui_stream= stream.unwrap();
			let write_stream= gui_stream.try_clone().unwrap();
			let read_stream= write_stream.try_clone().unwrap();
			let mut reader= std::io::BufReader::new(&read_stream);
			let mut peer_id= String::new(); std::io::BufRead::read_line(&mut reader,&mut peer_id).unwrap(); let peer_id= peer_id.trim();
			println!("peer identifies themself as: {}",peer_id);
			println!("authenticating peer by sending cookie");
			// TODO (open a stream to peer_id.onion, peer should accept and send the cookie back down this stream)
			println!("peer authenticated");
			let mut nick= String::new(); std::io::BufRead::read_line(&mut reader,&mut nick).unwrap(); let nick= nick.trim();
			println!("peer nickname: {}",nick);
			open_chat(gui_stream);
			/*
			let mut line= String::new();
			loop {
				line.clear();
				std::io::BufRead::read_line(&mut reader,&mut line).unwrap(); let line= line.trim();
				if line.trim()=="quit" {
					break;
				}
				std::io::Write::write(&mut write_stream,b"hello!\n").unwrap();
			}
			*/
		});
	}});
	println!("done listen");
}
fn do_menu(
	profile_dir:&String,
	thread_pool:GrowingThreadPool,
	tor_child_tx:tokio::sync::oneshot::Sender<TorChild>,
) {
	use gtk::prelude::*;
	// make profile_dir absolute so that tor can use it for a unix socket
	let cur_dir= std::env::current_dir().unwrap().into_os_string().into_string().unwrap();
	let profile_dir= format!("{}/{}",cur_dir,profile_dir);
	println!("profile_dir is: {}",profile_dir);
	let onion_key= get_onion_key(&profile_dir);
	println!("start tor");
	let rt= tokio::runtime::Runtime::new().unwrap();
	let (tor_child,listener,socks_port)= rt.block_on(async {
		let mut tor_child= TorChild::new(&profile_dir,&format!("{}/torrc",cur_dir)).await;
		println!("connected to control port");
		let listener= tor_child.make_onion_listener(onion_key).await;
		let socks_port= tor_child.enable_network().await;
		(tor_child,listener,socks_port)
	});
	println!("sending tor child");
	tor_child_tx.send(tor_child).unwrap();
	println!("starting listen");
	listen(listener,thread_pool.clone());
	let builder= open_glade("menu.glade");
	let button:gtk::Button= builder.get_object("connect_button").unwrap();
	let window:gtk::Window= builder.get_object("window").unwrap();
	window.resize(300,100);
	let thread_pool= std::rc::Rc::new(std::cell::RefCell::new(thread_pool));
	button.connect_clicked(move|_but| {
		connect_to_peer(socks_port,(*thread_pool).borrow().clone());
	});
	window.show_all();
}
fn main() {
	let rt= tokio::runtime::Runtime::new().unwrap();
	let thread_pool= GrowingThreadPool::new();
	gtk::init().unwrap();
	println!("gtk started");
	let thread_pool_rc= std::rc::Rc::new(std::cell::Cell::new(Some(thread_pool.clone())));
	let (tor_child_tx,tor_child_rx)= tokio::sync::oneshot::channel();
	let tor_child_tx= std::rc::Rc::new(std::cell::Cell::new(Some(tor_child_tx)));
	select_profile(move|prof|{do_menu(
		prof,
		(*thread_pool_rc).take().unwrap(),
		(*tor_child_tx).take().unwrap(),
	);});
	gtk::main();
	// keep tor_child here for RAII when gtk::main unblocks
	let _tor_child= rt.block_on(tor_child_rx);
	thread_pool.join();
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
