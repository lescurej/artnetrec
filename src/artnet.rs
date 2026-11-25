use std::net::UdpSocket;

pub const PORT: u16 = 6454;

pub fn socket() -> std::io::Result<UdpSocket> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.set_broadcast(true)?;
    Ok(socket)
}

pub fn packet(universe: u16, sequence: u8, slots: &[u8; 512]) -> [u8; 530] {
    let mut pkt = [0u8; 530];
    pkt[..8].copy_from_slice(b"Art-Net\0");
    pkt[8] = 0x00;
    pkt[9] = 0x50;
    pkt[11] = 14;
    pkt[12] = sequence;
    pkt[14] = universe as u8;
    pkt[15] = ((universe >> 8) & 0x7f) as u8;
    pkt[16] = 0x02;
    pkt[17] = 0x00;
    pkt[18..].copy_from_slice(slots);
    pkt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artdmx_header() {
        let mut slots = [0u8; 512];
        slots[0] = 0x68;
        slots[1] = 0xad;
        slots[2] = 0xff;
        slots[3] = 0xa0;
        let pkt = packet(0, 1, &slots);
        assert_eq!(&pkt[..8], b"Art-Net\0");
        assert_eq!(&pkt[8..12], &[0x00, 0x50, 0x00, 14]);
        assert_eq!(pkt[12], 1);
        assert_eq!(pkt[13], 0);
        assert_eq!(&pkt[14..18], &[0, 0, 0x02, 0x00]);
        assert_eq!(&pkt[18..24], &[0x68, 0xad, 0xff, 0xa0, 0, 0]);
        assert_eq!(pkt.len(), 530);
    }

    #[test]
    fn universe_splits_into_subuni_and_net() {
        let pkt = packet(256, 255, &[0; 512]);
        assert_eq!(pkt[14], 0);
        assert_eq!(pkt[15], 1);
        assert_eq!(pkt[12], 255);
    }
}
