use alloc::rc::Rc;
use blocking_network_stack::{IoError, Stack, Socket};
use embedded_io::{Read, Write};
use embedded_websocket::framer::{Framer, ReadResult, Stream};
use embedded_websocket::{
    WebSocketClient, WebSocketKey, WebSocketOptions, WebSocketSendMessageType,
};
use esp_hal::rng::Rng;
use esp_println::println;
use esp_radio::wifi::WifiDevice;
use smoltcp::wire::IpAddress;

pub struct WsClient {
    host: &'static str,
    ip: IpAddress,

    ws: WebSocketClient<Rng>,
    ws_key: Option<WebSocketKey>,

    ws_rx: [u8; 2048],
    ws_tx: [u8; 2048],
    frame_buf: [u8; 1024],
    read_cursor: usize,

    connected: bool,
}

impl WsClient {
    pub fn new(host: &'static str, ip: IpAddress) -> Self {
        let rng = Rng::new();

        Self {
            host,
            ip,
            ws: WebSocketClient::new_client(rng),
            ws_key: None,

            ws_rx: [0; 2048],
            ws_tx: [0; 2048],
            frame_buf: [0; 1024],
            read_cursor: 0,

            connected: false,
        }
    }

    // ---- handshake / connect ----
    pub fn connect<'a>(
        &mut self,
        socket: &mut Socket<'a, 'a, WifiDevice<'a>>,
    ) -> Result<(), &'static str> {
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
            .map_err(|_| "ws connect")?;

        socket.write_all(&self.ws_tx[..len]).map_err(|_| "ws write")?;

        let n = socket.read(&mut self.ws_rx).map_err(|_| "ws read")?;
        self.ws
            .client_accept(&key, &self.ws_rx[..n])
            .map_err(|_| "ws accept")?;

        self.ws_key = Some(key);
        self.connected = true;
        Ok(())
    }

    // ---- send if there is input ----
    pub fn poll_send<'a>(
        &mut self,
        socket: &mut Socket<'a, 'a, WifiDevice<'a>>,
        msg: Option<&[u8]>,
    ) {
        if !self.connected {
            return;
        }

        let msg = match msg {
            Some(m) => m,
            None => return,
        };

        let len = match self.ws.write(
            WebSocketSendMessageType::Text,
            true,
            msg,
            &mut self.ws_tx,
        ) {
            Ok(len) => len,
            Err(_) => return,
        };

        let _ = socket.write_all(&self.ws_tx[..len]);
    }

    // ---- try-recv ----
    pub fn poll_recv<'a>(
        &mut self,
        socket: &mut Socket<'a, 'a, WifiDevice<'a>>,
    ) {
        if !self.connected {
            return;
        }

        let mut ws_socket = WsSocket(socket);

        let mut framer = Framer::<_, embedded_websocket::Client>::new(
            &mut self.ws_rx,
            &mut self.read_cursor,
            &mut self.ws_tx,
            &mut self.ws,
        );

        match framer.read(&mut ws_socket, &mut self.frame_buf) {
            Ok(ReadResult::Text(txt)) => {
                println!("WS RX: {txt}");
            }
            Ok(_) => {}
            Err(_) => {}
        }
    }

    // ---- convenience poll ----
    pub fn poll<'a>(
        &mut self,
        socket: &mut Socket<'a, 'a, WifiDevice<'a>>,
        send: Option<&[u8]>,
    ) {
        self.poll_send(socket, send);
        self.poll_recv(socket);
    }
}

// ---- adapter ----
pub struct WsSocket<'a, 'b, 'c>(
    pub &'c mut Socket<'a, 'b, WifiDevice<'a>>
);

impl<'a, 'b, 'c> Stream<IoError> for WsSocket<'a, 'b, 'c>
where
    Socket<'a, 'b, WifiDevice<'a>>:
        Read<Error = IoError> + Write<Error = IoError>,
{
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, IoError> {
        self.0.read(buf)
    }

    fn write_all(&mut self, buf: &[u8]) -> Result<(), IoError> {
        self.0.write_all(buf)
    }
}
