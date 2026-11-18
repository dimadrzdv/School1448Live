//! Sockets for WebRTC media: one UDP socket and an ICE-TCP listener (RFC 4571 framing)
//! on the same port, for networks that block UDP.

use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;

use str0m::net::{Protocol, Transmit};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::mpsc;

pub enum TcpEvent {
    Opened(SocketAddr, mpsc::Sender<Vec<u8>>),
    Frame(SocketAddr, Vec<u8>),
    Closed(SocketAddr),
}

pub struct Net {
    pub udp: UdpSocket,
    pub tcp_rx: mpsc::Receiver<TcpEvent>,
    tcp_out: HashMap<SocketAddr, mpsc::Sender<Vec<u8>>>,
    /// Address of our host candidate; used as the destination of received packets.
    pub local: SocketAddr,
    send_errors: u64,
}

impl Net {
    pub async fn bind(port: u16, local: SocketAddr) -> io::Result<Self> {
        let bind: SocketAddr = ([0, 0, 0, 0], port).into();
        let udp = UdpSocket::bind(bind).await?;
        let listener = TcpListener::bind(bind).await?;
        let (tx, tcp_rx) = mpsc::channel(4096);
        tokio::spawn(accept_loop(listener, tx));
        Ok(Net { udp, tcp_rx, tcp_out: HashMap::new(), local, send_errors: 0 })
    }

    pub fn on_tcp_opened(&mut self, addr: SocketAddr, tx: mpsc::Sender<Vec<u8>>) {
        self.tcp_out.insert(addr, tx);
    }

    pub fn on_tcp_closed(&mut self, addr: SocketAddr) {
        self.tcp_out.remove(&addr);
    }

    pub fn send(&mut self, t: &Transmit) {
        match t.proto {
            Protocol::Udp => {
                if let Err(e) = self.udp.try_send_to(&t.contents, t.destination) {
                    self.send_errors += 1;
                    if self.send_errors.is_power_of_two() {
                        tracing::debug!("udp send: {e} ({} total)", self.send_errors);
                    }
                }
            }
            Protocol::Tcp => {
                if let Some(tx) = self.tcp_out.get(&t.destination) {
                    // A full queue means the client cannot keep up: drop, RTX/NACK recover.
                    let _ = tx.try_send(t.contents.to_vec());
                }
            }
            _ => {}
        }
    }
}

async fn accept_loop(listener: TcpListener, events: mpsc::Sender<TcpEvent>) {
    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                let _ = stream.set_nodelay(true);
                tokio::spawn(connection(stream, addr, events.clone()));
            }
            Err(e) => {
                tracing::warn!("ice-tcp accept: {e}");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }
}

async fn connection(stream: TcpStream, addr: SocketAddr, events: mpsc::Sender<TcpEvent>) {
    let (mut rd, mut wr) = stream.into_split();
    let (out_tx, mut out_rx) = mpsc::channel::<Vec<u8>>(512);
    if events.send(TcpEvent::Opened(addr, out_tx)).await.is_err() {
        return;
    }
    let writer = tokio::spawn(async move {
        let mut buf = Vec::with_capacity(2048);
        while let Some(frame) = out_rx.recv().await {
            buf.clear();
            buf.extend_from_slice(&(frame.len() as u16).to_be_bytes());
            buf.extend_from_slice(&frame);
            if wr.write_all(&buf).await.is_err() {
                break;
            }
        }
    });
    let mut len = [0u8; 2];
    loop {
        if rd.read_exact(&mut len).await.is_err() {
            break;
        }
        let n = u16::from_be_bytes(len) as usize;
        let mut frame = vec![0u8; n];
        if rd.read_exact(&mut frame).await.is_err() {
            break;
        }
        if events.send(TcpEvent::Frame(addr, frame)).await.is_err() {
            break;
        }
    }
    writer.abort();
    let _ = events.send(TcpEvent::Closed(addr)).await;
}
