use anyhow::bail;
use mac_address::MacAddress;
use std::{borrow::Cow, time::Duration};

#[derive(Debug, Clone)]
pub struct Heartbeat<'a> {
    duration: Cow<'a, [u8]>,
    id: Cow<'a, [u8]>,
    hops: Cow<'a, [u8]>,
    source: Cow<'a, [u8]>,
}

impl<'a> Heartbeat<'a> {
    pub fn new(duration: Duration, id: u32, source: MacAddress) -> Self {
        Self {
            duration: Cow::Owned(duration.as_millis().to_be_bytes().to_vec()),
            id: Cow::Owned(id.to_be_bytes().to_vec()),
            hops: Cow::Owned(0u32.to_be_bytes().to_vec()),
            source: Cow::Owned(source.bytes().to_vec()),
        }
    }

    pub fn duration(&self) -> Duration {
        Duration::from_millis(
            u64::try_from(u128::from_be_bytes(
                unsafe { self.duration.get_unchecked(0..16) }
                    .try_into()
                    .unwrap(),
            ))
            .unwrap(),
        )
    }

    pub fn id(&self) -> u32 {
        u32::from_be_bytes(unsafe { self.id.get_unchecked(0..4) }.try_into().unwrap())
    }

    pub fn hops(&self) -> u32 {
        u32::from_be_bytes(unsafe { self.hops.get_unchecked(0..4) }.try_into().unwrap())
    }

    pub fn source(&self) -> MacAddress {
        MacAddress::new(
            unsafe { self.source.get_unchecked(0..6) }
                .try_into()
                .unwrap(),
        )
    }
}

impl<'a> TryFrom<&'a [u8]> for Heartbeat<'a> {
    type Error = anyhow::Error;

    fn try_from(value: &'a [u8]) -> Result<Self, Self::Error> {
        let (Some(source), Some(hops), Some(id), Some(duration)) = (
            value.get(24..30),
            value.get(20..24),
            value.get(16..20),
            value.get(..16),
        ) else {
            bail!("cannot get members");
        };
        let duration = Cow::Borrowed(duration);
        let id = Cow::Borrowed(id);
        let hops = Cow::Borrowed(hops);
        let source = Cow::Borrowed(source);

        Ok(Self {
            duration,
            id,
            hops,
            source,
        })
    }
}

impl<'a> From<&Heartbeat<'a>> for Vec<Vec<u8>> {
    fn from(value: &Heartbeat<'a>) -> Self {
        let Some(hops) = value.hops.get(0..4) else {
            panic!("did not have hops")
        };
        let hops: [u8; 4] = hops.try_into().expect("convert");
        let hops = u32::from_be_bytes(hops) + 1;
        vec![
            value.duration.clone().into_owned(),
            value.id.clone().into_owned(),
            hops.to_be_bytes().to_vec(),
            value.source.clone().into_owned(),
        ]
    }
}

#[derive(Debug, Clone)]
pub struct HeartbeatReply<'a> {
    duration: Cow<'a, [u8]>,
    id: Cow<'a, [u8]>,
    hops: Cow<'a, [u8]>,
    source: Cow<'a, [u8]>,
    sender: Cow<'a, [u8]>,
}

impl<'a> HeartbeatReply<'a> {
    pub fn from_sender(value: &'a Heartbeat, sender: MacAddress) -> Self {
        Self {
            duration: value.duration.clone(),
            id: value.id.clone(),
            hops: value.hops.clone(),
            source: value.source.clone(),
            sender: Cow::Owned(sender.bytes().to_vec()),
        }
    }

    pub fn duration(&self) -> Duration {
        Duration::from_millis(
            u64::try_from(u128::from_be_bytes(
                unsafe { self.duration.get_unchecked(0..16) }
                    .try_into()
                    .unwrap(),
            ))
            .unwrap(),
        )
    }

    pub fn id(&self) -> u32 {
        u32::from_be_bytes(unsafe { self.id.get_unchecked(0..4) }.try_into().unwrap())
    }

    pub fn hops(&self) -> u32 {
        u32::from_be_bytes(unsafe { self.hops.get_unchecked(0..4) }.try_into().unwrap())
    }

    pub fn source(&self) -> MacAddress {
        MacAddress::new(
            unsafe { self.source.get_unchecked(0..6) }
                .try_into()
                .unwrap(),
        )
    }

    pub fn sender(&self) -> MacAddress {
        MacAddress::new(
            unsafe { self.sender.get_unchecked(0..6) }
                .try_into()
                .unwrap(),
        )
    }
}

impl<'a> TryFrom<&'a [u8]> for HeartbeatReply<'a> {
    type Error = anyhow::Error;

    fn try_from(value: &'a [u8]) -> Result<Self, Self::Error> {
        let (Some(sender), Some(source), Some(hops), Some(id), Some(duration)) = (
            value.get(30..36),
            value.get(24..30),
            value.get(20..24),
            value.get(16..20),
            value.get(..16),
        ) else {
            bail!("cannot get members");
        };
        let duration = Cow::Borrowed(duration);
        let id = Cow::Borrowed(id);
        let hops = Cow::Borrowed(hops);
        let source = Cow::Borrowed(source);
        let sender = Cow::Borrowed(sender);

        Ok(Self {
            duration,
            id,
            hops,
            source,
            sender,
        })
    }
}

impl<'a> From<&HeartbeatReply<'a>> for Vec<Vec<u8>> {
    fn from(value: &HeartbeatReply<'a>) -> Self {
        vec![
            value.duration.clone().into_owned(),
            value.id.clone().into_owned(),
            value.hops.clone().into_owned(),
            value.source.clone().into_owned(),
            value.sender.clone().into_owned(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::Heartbeat;
    use crate::messages::{control::Control, message::Message, packet_type::PacketType};
    use mac_address::MacAddress;
    use std::time::Duration;

    #[test]
    fn heartbeat_can_be_parsed_and_has_correct_members() {
        let pkt = vec![
            vec![1u8; 6],
            vec![2u8; 6],
            vec![0x30, 0x30],
            vec![0],
            vec![0],
            vec![4; 16],
            vec![0; 4],
            vec![1; 4],
            vec![2; 6],
        ];
        let apkt: Vec<u8> = pkt.iter().flat_map(|x| x.iter()).cloned().collect();
        let msg = Message::try_from(&apkt[..]).expect("is message");
        assert_eq!(msg.from().expect("has from"), MacAddress::new([2u8; 6]));
        assert_eq!(msg.to().expect("has to"), MacAddress::new([1u8; 6]));

        let rpkt: Vec<Vec<u8>> = (&msg).into();
        let pkt = vec![
            vec![1u8; 6],
            vec![2u8; 6],
            vec![0x30, 0x30],
            vec![0],
            vec![0],
            vec![4; 16],
            vec![0; 4],
            vec![1, 1, 1, 2],
            vec![2; 6],
        ];
        assert_eq!(pkt, rpkt);
    }

    #[test]
    fn create_hearbeat() {
        let msg = Message::new(
            [0; 6].into(),
            [255; 6].into(),
            PacketType::Control(Control::Heartbeat(Heartbeat::new(
                Duration::default(),
                0,
                [4; 6].into(),
            ))),
        );

        let pkt = vec![
            vec![255u8; 6],
            vec![0u8; 6],
            vec![0x30, 0x30],
            vec![0],
            vec![0],
            vec![0; 16],
            vec![0; 4],
            vec![0, 0, 0, 1],
            vec![4; 6],
        ];

        let to_vec: Vec<Vec<u8>> = (&msg).into();
        assert_eq!(to_vec, pkt);
    }

    #[test]
    fn heartbeat_reply_can_be_parsed_and_has_correct_members() {
        let pkt = vec![
            vec![1u8; 6],
            vec![2u8; 6],
            vec![0x30, 0x30],
            vec![0],
            vec![1],
            vec![4; 16],
            vec![0; 4],
            vec![1; 4],
            vec![2; 6],
            vec![3; 6],
        ];
        let apkt: Vec<u8> = pkt.iter().flat_map(|x| x.iter()).cloned().collect();
        let msg = Message::try_from(&apkt[..]).expect("is message");
        assert_eq!(msg.from().expect("has from"), MacAddress::new([2u8; 6]));
        assert_eq!(msg.to().expect("has to"), MacAddress::new([1u8; 6]));

        let rpkt: Vec<Vec<u8>> = (&msg).into();
        assert_eq!(pkt, rpkt);
    }

    #[test]
    fn heartbeat_try_from_too_short_fails() {
        let pkt = [0u8; 10];
        let res = Heartbeat::try_from(&pkt[..]);
        assert!(res.is_err());
    }

    #[test]
    fn heartbeat_reply_try_from_too_short_fails() {
        let pkt = [0u8; 20];
        let res = crate::messages::control::heartbeat::HeartbeatReply::try_from(&pkt[..]);
        assert!(res.is_err());
    }
}
