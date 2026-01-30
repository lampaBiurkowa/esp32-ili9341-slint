use alloc::rc::Rc;
use blocking_network_stack::{IoError, Stack};
use embedded_io::{Read, Write};
use embedded_websocket::framer::{Framer, ReadResult, Stream};
use embedded_websocket::{
    WebSocketClient, WebSocketKey, WebSocketOptions, WebSocketSendMessageType,
};
use esp_hal::rng::Rng;
use esp_println::println;
use esp_radio::wifi::WifiDevice;
use smoltcp::wire::IpAddress;

pub struct WsClient<'a> {
    pub stack: Rc<Stack<'a, WifiDevice<'a>>>,
    pub host: &'static str,
    pub ip: IpAddress,

    ws: WebSocketClient<Rng>,
    ws_key: Option<WebSocketKey>,

    tcp_rx: [u8; 1536],
    tcp_tx: [u8; 1536],

    ws_rx: [u8; 2048],
    ws_tx: [u8; 2048],
    frame_buf: [u8; 1024],
    read_cursor: usize,

    connected: bool,
}

impl<'a> WsClient<'a> {
    pub fn new(stack: Rc<Stack<'a, WifiDevice<'a>>>, host: &'static str, ip: IpAddress) -> Self {
        let rng = Rng::new();

        Self {
            stack,
            host,
            ip,
            ws: WebSocketClient::new_client(rng.clone()),
            ws_key: None,
            tcp_rx: [0; 1536],
            tcp_tx: [0; 1536],
            ws_rx: [0; 2048],
            ws_tx: [0; 2048],
            frame_buf: [0; 1024],
            connected: false,
            read_cursor: 0,
        }
    }

    pub fn run(&'a mut self) -> Result<(), &'static str> {
        // --- connect logic (INLINE, not via connect()) ---
        let mut socket = self.stack.get_socket(&mut self.tcp_rx, &mut self.tcp_tx);
        socket.work();
        socket.open(self.ip, 8765).map_err(|_| "open failed")?;

        let opts = WebSocketOptions {
            path: "/",
            host: self.host,
            origin: "",
            sub_protocols: None,
            additional_headers: None,
        };

        let (len, key) = self
            .ws
            .client_connect(&opts, &mut self.ws_tx)
            .inspect_err(|e| println!("err {e}"))
            .map_err(|_| "ws connect")?;

        socket.write(&self.ws_tx[..len]).map_err(|_| "ws write")?;
        socket.flush().map_err(|_| "ws flush")?;

        let n = socket
            .read(&mut self.ws_rx)
            .inspect_err(|e| println!("err {e}"))
            .map_err(|_| "ws read")?;
        self.ws
            .client_accept(&key, &self.ws_rx[..n])
            .map_err(|_| "ws accept")?;

        self.connected = true;

        // --- send ---
        let len = self
            .ws
            .write(
                WebSocketSendMessageType::Text,
                true,
                b"hello from esp32",
                &mut self.ws_tx,
            )
            .map_err(|_| "ws frame")?;

        socket.write(&self.ws_tx[..len]).map_err(|_| "ws write")?;
        socket.flush().map_err(|_| "ws flush")?;
        let mut ws_socket = WsSocket(socket);
        // --- poll loop ---
        loop {
            let mut framer = Framer::<_, embedded_websocket::Client>::new(
                &mut self.ws_rx,
                &mut self.read_cursor,
                &mut self.ws_tx,
                &mut self.ws,
            );

            match framer.read(&mut ws_socket, &mut self.frame_buf) {
                Ok(x) => match x {
                    ReadResult::Text(x) => println!("{x}"),
                    _ => ()//println!("Got non-text"),
                },
                Err(e) => println!("Failed to read response: {e:?}"),
            }

            self.stack.work();
        }
    }
}

pub struct WsSocket<'a, 'b>(pub blocking_network_stack::Socket<'a, 'b, WifiDevice<'a>>);

impl<'a, 'b> Stream<IoError> for WsSocket<'a, 'b>
where
    blocking_network_stack::Socket<'a, 'b, WifiDevice<'a>>:
        Read<Error = IoError> + Write<Error = IoError>,
{
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, IoError> {
        self.0.read(buf)
    }

    fn write_all(&mut self, buf: &[u8]) -> Result<(), IoError> {
        self.0.write_all(buf)
    }
}
