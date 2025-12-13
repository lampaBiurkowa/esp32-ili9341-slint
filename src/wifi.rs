use blocking_network_stack::Stack;
use esp_radio::wifi::{ClientConfig, Interfaces, ModeConfig, ScanConfig, WifiController, WifiDevice};
use smoltcp::{
    iface::{Interface, SocketSet, SocketStorage},
    socket::dhcpv4,
    wire::{DhcpOption, EthernetAddress, HardwareAddress},
};

pub(crate) struct Wifi<'a> {
    controller: WifiController<'a>,
    pub(crate) interfaces: Interfaces<'a>,
}

impl<'a> Wifi<'a> {
    pub(crate) fn new(
        wifi: esp_hal::peripherals::WIFI<'a>,
        radio: &'a esp_radio::Controller,
        ssid: &str,
        password: &str,
    ) -> Self {
        let (mut controller, interfaces) =
            esp_radio::wifi::new(radio, wifi, Default::default())
                .expect("wifi init failed");

        controller
            .set_config(&ModeConfig::Client(
                ClientConfig::default()
                    .with_ssid(ssid.into())
                    .with_password(password.into()),
            ))
            .unwrap();

        Self { controller, interfaces }
    }

    pub(crate) fn initialize(&mut self) {
        self.start();
        self.scan();
        self.connect();
    }

    fn start(&mut self) {
        self.controller.start().unwrap();
    }

    fn scan(&mut self) {
        let cfg = ScanConfig::default().with_max(10);
        let res = self.controller.scan_with_config(cfg).unwrap();
        for ap in res {
            esp_println::println!("{:?}", ap);
        }
    }

    fn connect(&mut self) {
        self.controller.connect().unwrap();
        loop {
            match self.controller.is_connected() {
                Ok(true) => break,
                Ok(false) => {}
                Err(e) => panic!("{:?}", e),
            }
        }
        esp_println::println!("Connected: {:?}", self.controller.is_connected());
    }
}

pub fn create_interface(device: &mut WifiDevice) -> Interface {
    Interface::new(
        smoltcp::iface::Config::new(HardwareAddress::Ethernet(
            EthernetAddress::from_bytes(&device.mac_address()),
        )),
        device,
        timestamp(),
    )
}

fn timestamp() -> smoltcp::time::Instant {
    smoltcp::time::Instant::from_micros(
        esp_hal::time::Instant::now()
            .duration_since_epoch()
            .as_micros() as i64,
    )
}

pub fn init_sockets_with_dhcp<'a>(
    entries: &'a mut [SocketStorage<'a>],
) -> SocketSet<'a> {
    let mut set = SocketSet::new(entries);

    let mut dhcp = dhcpv4::Socket::new();
    dhcp.set_outgoing_options(&[DhcpOption {
        kind: 12,
        data: b"implRust",
    }]);
    set.add(dhcp);

    set
}

pub fn build_stack<'a>(
    mut device: WifiDevice<'a>,
    socket_entries: &'a mut [SocketStorage<'a>],
    now_fn: fn() -> u64,
    rng_seed: u32,
) -> Stack<'a, WifiDevice<'a>>
{
    let iface = create_interface(&mut device);
    let sockets = init_sockets_with_dhcp(socket_entries);

    Stack::new(iface, device, sockets, now_fn, rng_seed)
}

pub fn obtain_ip(stack: &Stack<'_, WifiDevice<'_>>) {
    esp_println::println!("Wait for IP address");
    loop {
        stack.work();
        if stack.is_iface_up() {
            esp_println::println!("IP acquired: {:?}", stack.get_ip_info());
            break;
        }
    }
}