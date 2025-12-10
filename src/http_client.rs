use alloc::string::String;
use alloc::format;
use blocking_network_stack::Stack;
use embedded_io::{Read, Write};
use esp_hal::time::{Duration, Instant};
use esp_println::println;
use esp_radio::wifi::WifiDevice;
use smoltcp::wire::IpAddress;

#[derive(Copy, Clone)]
pub enum Method {
    Get,
    Post,
    Put,
    Delete,
    Patch,
}

impl Method {
    fn as_str(&self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Delete => "DELETE",
            Method::Patch => "PATCH",
        }
    }
}

pub struct HttpClient<'a> {
    pub stack: &'a mut Stack<'a, WifiDevice<'a>>,
    pub host: &'a str,
    pub ip: IpAddress,
}

impl<'a> HttpClient<'a> {
    pub fn new(
        stack: &'a mut Stack<'a, WifiDevice<'a>>,
        host: &'a str,
        ip: IpAddress,
    ) -> Self {
        Self { stack, host, ip }
    }

    pub fn request(
        &mut self,
        method: Method,
        route: &str,
        rx_buf: &'a mut [u8],
        tx_buf: &'a mut [u8],
        body: Option<&[u8]>,
        timeout_secs: u64,
    ) -> Result<String, &'static str> {
        let mut socket = self.stack.get_socket(rx_buf, tx_buf);
        socket.work();

        socket.open(self.ip, 80).map_err(|_| "open failed")?;

        let method_str = method.as_str();
        let body_len = body.map(|b| b.len()).unwrap_or(0);

        let mut request = format!(
            "{} {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: esp32-rust\r\n",
            method_str,
            route,
            self.host
        );

        if let Some(_) = body {
            request.push_str(&format!("Content-Length: {}\r\n", body_len));
            request.push_str("Content-Type: application/json\r\n");
        }

        request.push_str("Connection: close\r\n\r\n");

        socket.write(request.as_bytes()).map_err(|_| "write failed")?;

        if let Some(bytes) = body {
            socket.write(bytes).map_err(|_| "body write failed")?;
        }

        socket.flush().map_err(|_| "flush failed")?;

        let deadline = Instant::now() + Duration::from_secs(timeout_secs);
        let mut out = String::new();
        let mut temp = [0u8; 256];

        loop {
            match socket.read(&mut temp) {
                Ok(0) => break, // EOF
                Ok(n) => {
                    if let Ok(s) = core::str::from_utf8(&temp[..n]) {
                        out.push_str(s);
                    } else {
                        return Err("utf8 error");
                    }
                }
                Err(_) => break,
            }

            if Instant::now() > deadline {
                println!("http timeout");
                break;
            }
        }

        socket.disconnect();

        let end_deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < end_deadline {
            socket.work();
        }

        Ok(out)
    }
}
