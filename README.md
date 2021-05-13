# p2p-tor-chat

This is a simple program to connect together two peers via TCP streams forwarded through Tor hidden services. Run with ```cargo run```, which should spend a long time compiling the first time you run it. If you run two instances of this program, you can pass the onion address from one instance to the other instance in order to connect both of them. You can do this from any 2 computers that can connect to the Tor network, since Tor handles all the NAT traversal.
