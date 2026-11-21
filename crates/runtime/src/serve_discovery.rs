//! Where a listener for the network is reached, and announcing it there
//! (IC-990).
//!
//! An app that serves the floor's tablets (`net.listen`, IC-987) used to have
//! no way to say where it was: the person had to find this computer's address
//! in its network settings and type it on every tablet. Two things close that:
//!
//! - **The addresses.** [`local_network_addresses`] lists this computer's
//!   private IPv4 addresses (RFC 1918, and link-local for a cable between two
//!   machines), its main connection first. A public address is never one of
//!   them, and nothing here reads a hardware (MAC) address. IPv4 only, because
//!   a listener for the network binds every IPv4 address and no IPv6 one.
//! - **The announcement.** A [`Discovery`] answers multicast DNS (DNS-SD, the
//!   way printers are found) for a listener under the app's own name, through
//!   a [`Responder`]: mdns-sd on the network, or -- when `KRATE_ANNOUNCE_TO`
//!   names a file -- that file, a test's stand-in that sends nothing. Each
//!   announcement is withdrawn when its listener closes, and every one when
//!   the run ends (the [`Discovery`] drops with the host).
//!
//! The grant is judged before this module is reached: the host refuses both
//! for a listener that is not for the network, which only the person's yes
//! for `net.listen:<port>` opens.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::net::{IpAddr, Ipv4Addr, UdpSocket};
use std::path::PathBuf;
use std::time::Duration;

/// Whether devices on the same local network reach this computer at `ip`:
/// a private address (10/8, 172.16/12, 192.168/16) or a link-local one
/// (169.254/16, two machines on a cable with no router between). Never
/// loopback, never a public address.
pub(crate) fn is_local_network(ip: Ipv4Addr) -> bool {
    ip.is_private() || ip.is_link_local()
}

/// One address the system lists for an interface: the input to [`choose`].
#[derive(Debug, Clone)]
pub(crate) struct Seen {
    pub name: String,
    pub ip: IpAddr,
    /// The system says the interface is down.
    pub down: bool,
}

/// This computer's addresses on the local network, the main connection's
/// first. Empty when it is on no local network (offline, or on a public
/// address only).
pub(crate) fn local_network_addresses() -> Vec<Ipv4Addr> {
    let seen: Vec<Seen> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .map(|i| Seen {
            name: i.name.clone(),
            ip: i.ip(),
            down: matches!(
                i.oper_status,
                if_addrs::IfOperStatus::Down
                    | if_addrs::IfOperStatus::NotPresent
                    | if_addrs::IfOperStatus::LowerLayerDown
            ),
        })
        .collect();
    choose(&seen, main_route_address())
}

/// The address this computer's default route leaves from. A UDP socket
/// "connected" to an address sends nothing; the system only picks the source
/// it would use. 192.0.2.1 is a documentation address (TEST-NET-1) nothing
/// answers, so no host is ever contacted.
fn main_route_address() -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect((Ipv4Addr::new(192, 0, 2, 1), 9)).ok()?;
    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(ip) if is_local_network(ip) => Some(ip),
        _ => None,
    }
}

/// Interfaces a tablet on the floor is unlikely to reach: containers,
/// virtual machines, VPN tunnels, Apple's peer-to-peer links. Listed last,
/// never dropped -- a person may really be serving over one.
fn looks_virtual(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    [
        "docker",
        "br-",
        "veth",
        "virbr",
        "vmnet",
        "vboxnet",
        "bridge",
        "utun",
        "tun",
        "tap",
        "awdl",
        "llw",
        "zt",
        "tailscale",
        "wg",
        "vethernet",
        "virtualbox",
        "vmware",
        "hyper-v",
    ]
    .iter()
    .any(|prefix| name.starts_with(prefix))
}

/// The local-network IPv4 addresses among `seen`, each once: the main
/// connection's (`main`) first, then the ones on real-looking interfaces,
/// then the virtual-looking ones, otherwise in the system's order.
pub(crate) fn choose(seen: &[Seen], main: Option<Ipv4Addr>) -> Vec<Ipv4Addr> {
    let mut chosen: Vec<(bool, bool, usize, Ipv4Addr)> = Vec::new();
    for (at, s) in seen.iter().enumerate() {
        let IpAddr::V4(ip) = s.ip else { continue };
        if s.down || !is_local_network(ip) || chosen.iter().any(|c| c.3 == ip) {
            continue;
        }
        chosen.push((Some(ip) != main, looks_virtual(&s.name), at, ip));
    }
    chosen.sort();
    chosen.into_iter().map(|c| c.3).collect()
}

/// What to open in a browser on another device for `host` and `port`.
pub(crate) fn url(host: &str, port: u16) -> String {
    format!("http://{host}:{port}/")
}

/// The service type an app asked for, as DNS-SD writes it:
/// `_http._tcp` becomes `_http._tcp.local.`. One to fifteen lowercase
/// letters, digits and hyphens (RFC 6335), at least one letter, no hyphen at
/// either end or two together, over TCP -- a listener is TCP. The words say
/// what to write instead.
pub(crate) fn service_type(asked: &str) -> Result<String, String> {
    let words = || {
        format!(
            "`{asked}` is not a service type: use _http._tcp for web pages, or your own as an \
             underscore, 1 to 15 lowercase letters, digits and hyphens, then ._tcp \
             (_shopfloor._tcp)"
        )
    };
    let trimmed = asked.trim().trim_end_matches('.');
    let trimmed = trimmed.strip_suffix(".local").unwrap_or(trimmed);
    let Some(name) = trimmed
        .strip_suffix("._tcp")
        .and_then(|rest| rest.strip_prefix('_'))
    else {
        return Err(words());
    };
    let good = (1..=15).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && name.bytes().any(|b| b.is_ascii_lowercase())
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--");
    if !good {
        return Err(words());
    }
    Ok(format!("_{name}._tcp.local."))
}

/// The name devices list an app under: its own name as the person
/// consented to it, at most 63 bytes (a DNS label), or "Krate app".
pub(crate) fn instance_name(app_name: &str) -> String {
    let clean: String = app_name
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .to_string();
    let mut name = String::new();
    for c in clean.chars() {
        if name.len() + c.len_utf8() > 63 {
            break;
        }
        name.push(c);
    }
    let name = name.trim().to_string();
    if name.is_empty() {
        "Krate app".to_string()
    } else {
        name
    }
}

/// The host name a device reaches the app at, without `.local`: the app's
/// name in lowercase letters, digits and hyphens ("Shop Floor" is
/// `shop-floor`), at most 63 of them, or `krate-app`.
pub(crate) fn host_label(app_name: &str) -> String {
    let mut label = String::new();
    for c in app_name.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            label.push(c);
        } else if !label.is_empty() && !label.ends_with('-') {
            label.push('-');
        }
    }
    label.truncate(63);
    let label = label.trim_matches('-').to_string();
    if label.is_empty() {
        "krate-app".to_string()
    } else {
        label
    }
}

/// One listener, as the network is told about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Announcement {
    /// The name devices list it under ([`instance_name`]).
    pub name: String,
    /// `shop-floor.local`, without the final dot.
    pub host: String,
    /// `_http._tcp.local.` ([`service_type`]).
    pub service_type: String,
    pub port: u16,
    /// The addresses [`local_network_addresses`] gave.
    pub ips: Vec<Ipv4Addr>,
}

/// What answers for announcements: the network, or a test's stand-in.
pub(crate) trait Responder: Send {
    /// Start answering for `a`. Returns the key to withdraw it by.
    fn announce(&mut self, a: &Announcement) -> Result<String, String>;
    /// Stop answering for what `key` announced, telling the network it is
    /// gone.
    fn withdraw(&mut self, key: &str);
}

/// Multicast DNS through mdns-sd. The daemon -- one thread -- starts with
/// the first announcement and is shut down (every announcement withdrawn
/// with a goodbye) when this drops.
pub(crate) struct Mdns {
    daemon: Option<mdns_sd::ServiceDaemon>,
    port: u16,
    /// A test's: this computer's loopback alone, on a port of its own, so
    /// nothing reaches the network.
    loopback_only: bool,
}

impl Mdns {
    /// The network, on the multicast DNS port.
    pub(crate) fn network() -> Self {
        Self {
            daemon: None,
            port: mdns_sd::MDNS_PORT,
            loopback_only: false,
        }
    }

    /// This computer alone, on `port`: what a test announces on.
    #[cfg(test)]
    pub(crate) fn loopback(port: u16) -> Self {
        Self {
            daemon: None,
            port,
            loopback_only: true,
        }
    }
}

impl Responder for Mdns {
    fn announce(&mut self, a: &Announcement) -> Result<String, String> {
        if self.daemon.is_none() {
            let daemon =
                mdns_sd::ServiceDaemon::new_with_port(self.port).map_err(|e| e.to_string())?;
            // Only the interfaces the announcement is for: not a VPN or a
            // container the app is not reached through, and no IPv6, which
            // the listener does not bind.
            daemon
                .disable_interface(mdns_sd::IfKind::All)
                .map_err(|e| e.to_string())?;
            self.daemon = Some(daemon);
        }
        let daemon = self.daemon.as_ref().expect("started above");
        let ips: Vec<IpAddr> = a.ips.iter().map(|ip| IpAddr::V4(*ip)).collect();
        if self.loopback_only {
            daemon.enable_interface(mdns_sd::IfKind::LoopbackV4)
        } else {
            daemon.enable_interface(
                ips.iter()
                    .map(|ip| mdns_sd::IfKind::Addr(*ip))
                    .collect::<Vec<_>>(),
            )
        }
        .map_err(|e| e.to_string())?;
        let info = mdns_sd::ServiceInfo::new(
            &a.service_type,
            &a.name,
            &format!("{}.", a.host),
            &ips[..],
            a.port,
            &[("path", "/")][..],
        )
        .map_err(|e| e.to_string())?;
        let key = info.get_fullname().to_string();
        daemon.register(info).map_err(|e| e.to_string())?;
        Ok(key)
    }

    fn withdraw(&mut self, key: &str) {
        if let Some(daemon) = &self.daemon {
            let _ = daemon.unregister(key);
        }
    }
}

impl Drop for Mdns {
    fn drop(&mut self) {
        if let Some(daemon) = self.daemon.take() {
            // Shutting down withdraws every announcement with a goodbye; a
            // moment for those to leave before the app's process may end.
            if let Ok(done) = daemon.shutdown() {
                let _ = done.recv_timeout(Duration::from_secs(1));
            }
        }
    }
}

/// `KRATE_ANNOUNCE_TO`: announcements written to a file, one line each,
/// instead of sent. Nothing reaches the network. Lines are tab-separated:
/// `announce <key> <host> <port> <ip,ip>` and `withdraw <key>`.
struct ToFile {
    path: PathBuf,
}

impl ToFile {
    fn line(&self, text: &str) -> Result<(), String> {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .and_then(|mut f| writeln!(f, "{text}"))
            .map_err(|e| format!("{}: {e}", self.path.display()))
    }
}

impl Responder for ToFile {
    fn announce(&mut self, a: &Announcement) -> Result<String, String> {
        let key = format!("{}.{}", a.name, a.service_type);
        let ips: Vec<String> = a.ips.iter().map(|ip| ip.to_string()).collect();
        self.line(&format!(
            "announce\t{key}\t{}\t{}\t{}",
            a.host,
            a.port,
            ips.join(",")
        ))?;
        Ok(key)
    }

    fn withdraw(&mut self, key: &str) {
        let _ = self.line(&format!("withdraw\t{key}"));
    }
}

/// The responder a run announces through: the network, or the file
/// `KRATE_ANNOUNCE_TO` names.
fn system_responder() -> Box<dyn Responder> {
    match std::env::var_os("KRATE_ANNOUNCE_TO") {
        Some(path) if !path.is_empty() => Box::new(ToFile {
            path: PathBuf::from(path),
        }),
        _ => Box::new(Mdns::network()),
    }
}

/// A run's announcements, by listener handle. Nothing starts until the first
/// one; dropped with the run, it withdraws every one.
pub(crate) struct Discovery {
    responder: Option<Box<dyn Responder>>,
    make: fn() -> Box<dyn Responder>,
    announced: BTreeMap<u64, String>,
}

impl Default for Discovery {
    fn default() -> Self {
        Self::new()
    }
}

impl Discovery {
    pub(crate) fn new() -> Self {
        Self {
            responder: None,
            make: system_responder,
            announced: BTreeMap::new(),
        }
    }

    /// Announce through `responder` instead: a test's.
    #[cfg(test)]
    pub(crate) fn with_responder(responder: Box<dyn Responder>) -> Self {
        Self {
            responder: Some(responder),
            make: system_responder,
            announced: BTreeMap::new(),
        }
    }

    /// Announce `listener` as `a`, replacing what it announced before.
    pub(crate) fn announce(&mut self, listener: u64, a: &Announcement) -> Result<(), String> {
        let responder = self.responder.get_or_insert_with(self.make);
        if let Some(old) = self.announced.remove(&listener) {
            responder.withdraw(&old);
        }
        let key = responder.announce(a)?;
        self.announced.insert(listener, key);
        Ok(())
    }

    /// The listener closed: withdraw what it announced, if anything.
    pub(crate) fn withdraw(&mut self, listener: u64) {
        if let (Some(key), Some(responder)) =
            (self.announced.remove(&listener), self.responder.as_mut())
        {
            responder.withdraw(&key);
        }
    }

    /// How many announcements are live: what a test asks.
    #[cfg(test)]
    pub(crate) fn live(&self) -> usize {
        self.announced.len()
    }
}

impl Drop for Discovery {
    fn drop(&mut self) {
        let listeners: Vec<u64> = self.announced.keys().copied().collect();
        for listener in listeners {
            self.withdraw(listener);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// A responder that remembers what it was told.
    #[derive(Clone, Default)]
    pub(crate) struct Heard(pub Arc<Mutex<Vec<String>>>);

    impl Responder for Heard {
        fn announce(&mut self, a: &Announcement) -> Result<String, String> {
            let key = format!("{}.{}", a.name, a.service_type);
            self.0
                .lock()
                .unwrap()
                .push(format!("announce {key} {} {} {:?}", a.host, a.port, a.ips));
            Ok(key)
        }
        fn withdraw(&mut self, key: &str) {
            self.0.lock().unwrap().push(format!("withdraw {key}"));
        }
    }

    fn seen(name: &str, ip: &str) -> Seen {
        Seen {
            name: name.to_string(),
            ip: ip.parse().expect(ip),
            down: false,
        }
    }

    #[test]
    fn only_private_ipv4_addresses_are_offered_main_connection_first() {
        let all = [
            seen("lo0", "127.0.0.1"),
            seen("lo0", "::1"),
            seen("docker0", "172.17.0.1"),
            seen("en0", "fe80::1"),
            seen("en1", "10.0.0.7"),
            seen("en0", "192.168.1.18"),
            seen("en5", "203.0.113.9"),
            seen("en6", "8.8.4.4"),
            seen("en7", "169.254.10.2"),
            seen("utun3", "100.101.102.103"),
            seen("en8", "172.32.0.1"),
            seen("en0", "192.168.1.18"),
        ];
        let main: Ipv4Addr = "192.168.1.18".parse().unwrap();
        assert_eq!(
            choose(&all, Some(main)),
            ["192.168.1.18", "10.0.0.7", "169.254.10.2", "172.17.0.1"]
                .map(|ip| ip.parse::<Ipv4Addr>().unwrap())
        );
        // No main connection known: real interfaces first, in order.
        assert_eq!(
            choose(&all, None),
            ["10.0.0.7", "192.168.1.18", "169.254.10.2", "172.17.0.1"]
                .map(|ip| ip.parse::<Ipv4Addr>().unwrap())
        );
        // A computer on a public address alone, or with a down interface,
        // offers nothing.
        let mut down = seen("en0", "192.168.1.18");
        down.down = true;
        assert!(choose(&[seen("en0", "203.0.113.9"), down], None).is_empty());
        for public in [
            "203.0.113.9",
            "8.8.8.8",
            "100.64.0.1",
            "172.32.0.1",
            "127.0.0.1",
        ] {
            assert!(!is_local_network(public.parse().unwrap()), "{public}");
        }
    }

    #[test]
    fn this_computers_own_addresses_are_all_private() {
        // Judged by the standard library's own definitions, not by the
        // predicate under test.
        for ip in local_network_addresses() {
            assert!(ip.is_private() || ip.is_link_local(), "{ip}");
        }
    }

    #[test]
    fn a_service_type_is_dns_sds_or_refused_in_words() {
        for (asked, want) in [
            ("_http._tcp", "_http._tcp.local."),
            ("_http._tcp.local", "_http._tcp.local."),
            ("_http._tcp.local.", "_http._tcp.local."),
            (" _shop-floor2._tcp ", "_shop-floor2._tcp.local."),
            ("_abcdefghijklmno._tcp", "_abcdefghijklmno._tcp.local."),
        ] {
            assert_eq!(service_type(asked).as_deref(), Ok(want), "{asked}");
        }
        for bad in [
            "",
            "http",
            "_http._udp",
            "_http",
            "_._tcp",
            "_HTTP._tcp",
            "_-http._tcp",
            "_http-._tcp",
            "_ht--tp._tcp",
            "_123._tcp",
            "_abcdefghijklmnop._tcp",
            "_a b._tcp",
            "_a.b._tcp",
        ] {
            let words = service_type(bad).expect_err(bad);
            assert!(words.contains("_http._tcp for web pages"), "{words}");
        }
    }

    #[test]
    fn names_come_from_the_app() {
        assert_eq!(host_label("Shop Floor"), "shop-floor");
        assert_eq!(host_label("  Line 3 -- Checklist!  "), "line-3-checklist");
        assert_eq!(host_label("Café"), "caf");
        assert_eq!(host_label("日本"), "krate-app");
        assert_eq!(host_label(""), "krate-app");
        assert_eq!(host_label(&"a".repeat(80)).len(), 63);
        assert_eq!(instance_name("  Shop Floor\n"), "Shop Floor");
        assert_eq!(instance_name(""), "Krate app");
        assert!(instance_name(&"é".repeat(40)).len() <= 63);
        assert_eq!(
            url("shop-floor.local", 8080),
            "http://shop-floor.local:8080/"
        );
    }

    #[test]
    fn an_announcement_is_withdrawn_when_replaced_closed_or_dropped() {
        let heard = Heard::default();
        let mut discovery = Discovery::with_responder(Box::new(heard.clone()));
        let a = |ty: &str| Announcement {
            name: "Shop Floor".into(),
            host: "shop-floor.local".into(),
            service_type: ty.into(),
            port: 8080,
            ips: vec!["192.168.1.18".parse().unwrap()],
        };
        discovery.announce(1, &a("_http._tcp.local.")).unwrap();
        discovery.announce(1, &a("_shop._tcp.local.")).unwrap();
        discovery.announce(2, &a("_http._tcp.local.")).unwrap();
        assert_eq!(discovery.live(), 2);
        discovery.withdraw(1);
        discovery.withdraw(1);
        discovery.withdraw(7);
        assert_eq!(discovery.live(), 1);
        drop(discovery);
        assert_eq!(
            *heard.0.lock().unwrap(),
            [
                "announce Shop Floor._http._tcp.local. shop-floor.local 8080 [192.168.1.18]",
                "withdraw Shop Floor._http._tcp.local.",
                "announce Shop Floor._shop._tcp.local. shop-floor.local 8080 [192.168.1.18]",
                "announce Shop Floor._http._tcp.local. shop-floor.local 8080 [192.168.1.18]",
                "withdraw Shop Floor._shop._tcp.local.",
                "withdraw Shop Floor._http._tcp.local.",
            ]
        );
    }

    /// The real responder, seen by a real resolver: mdns-sd announces on
    /// this computer's loopback on a port of its own, a second daemon browses
    /// for it, finds it with its address and port, and sees it go when the
    /// announcement is withdrawn. Off unless `KRATE_MDNS_LIVE_TEST=1`:
    /// multicast, even on loopback, can ask macOS for local network access
    /// in a session that never had it.
    #[test]
    fn a_resolver_finds_the_announcement_and_sees_it_withdrawn() {
        if std::env::var("KRATE_MDNS_LIVE_TEST").as_deref() != Ok("1") {
            eprintln!("skipped: set KRATE_MDNS_LIVE_TEST=1 to announce on loopback multicast");
            return;
        }
        let port = {
            let s = UdpSocket::bind("127.0.0.1:0").expect("a free port");
            s.local_addr().expect("addr").port()
        };
        let browser = mdns_sd::ServiceDaemon::new_with_port(port).expect("browser");
        browser.disable_interface(mdns_sd::IfKind::All).unwrap();
        browser
            .enable_interface(mdns_sd::IfKind::LoopbackV4)
            .unwrap();
        let events = browser.browse("_krateprobe._tcp.local.").expect("browse");
        let mut discovery = Discovery::with_responder(Box::new(Mdns::loopback(port)));
        discovery
            .announce(
                1,
                &Announcement {
                    name: "Shop Floor".into(),
                    host: "shop-floor.local".into(),
                    service_type: "_krateprobe._tcp.local.".into(),
                    port: 18_080,
                    ips: vec![Ipv4Addr::LOCALHOST],
                },
            )
            .expect("announce");
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        let mut found = None;
        while found.is_none() && std::time::Instant::now() < deadline {
            if let Ok(mdns_sd::ServiceEvent::ServiceResolved(info)) =
                events.recv_timeout(Duration::from_millis(500))
            {
                found = Some(info);
            }
        }
        let info = found.expect("the resolver finds the announcement");
        assert_eq!(info.get_fullname(), "Shop Floor._krateprobe._tcp.local.");
        assert_eq!(info.get_hostname(), "shop-floor.local.");
        assert_eq!(info.get_port(), 18_080);
        assert!(info.get_addresses_v4().contains(&Ipv4Addr::LOCALHOST));
        discovery.withdraw(1);
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        let mut gone = false;
        while !gone && std::time::Instant::now() < deadline {
            gone = matches!(
                events.recv_timeout(Duration::from_millis(500)),
                Ok(mdns_sd::ServiceEvent::ServiceRemoved(_, name))
                    if name == "Shop Floor._krateprobe._tcp.local."
            );
        }
        assert!(gone, "the resolver sees the announcement withdrawn");
        let _ = browser.shutdown();
    }
}
