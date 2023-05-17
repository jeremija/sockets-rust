# sockets-rust

_status: pre-alpha_

This project allows local ports hidden behind a NAT to be exposed in the cloud
by using a centralized service.

Currently only raw TCP connections are supported, but there's no reason why
HTTP(s) or WebSocket connections should not be supported.

Sample usage:

```console
$ nc --broker --listen -p 8000
```

```console
$ cargo run --bin server
```

```console
$ cargo run --bin client -- --expose 127.0.0.1:8000
   Compiling sockets v0.1.0 (/home/jere/src/sockets)
    Finished dev [unoptimized + debuginfo] target(s) in 2.27s
     Running `target/debug/client --expose '127.0.0.1:8000'`
header * connection
header * upgrade
header * sec-websocket-accept
exposed with tunnel id fdc7b471-df4c-4af4-81e4-918e77ba7617, url: localhost:41159
```

Then run this command in two different terminals

```
$ telnet localhost 41159
Trying ::1...
Connection failed: Connection refused
Trying 127.0.0.1...
Connected to localhost.
Escape character is '^]'.
```

You'll notice that the the two telnet clients can communicate with each other.

```
$ telnet localhost 41159
Trying ::1...
Connection failed: Connection refused
Trying 127.0.0.1...
Connected to localhost.
Escape character is '^]'.
test1
test2
```

```
$ telnet localhost 41159
Trying ::1...
Connection failed: Connection refused
Trying 127.0.0.1...
Connected to localhost.
Escape character is '^]'.
test1
test2
```

# License

MIT
