use mac_address::MacAddress;

pub trait NetworkInterface {
    fn mac_address(&self) -> MacAddress;
}

#[cfg(test)]
mod tests {
    use super::NetworkInterface;
    use mac_address::MacAddress;

    struct Dummy {
        mac: MacAddress,
    }

    impl NetworkInterface for Dummy {
        fn mac_address(&self) -> MacAddress {
            self.mac
        }
    }

    #[test]
    fn dummy_network_interface_returns_mac() {
        let mac: MacAddress = [1, 2, 3, 4, 5, 6].into();
        let d = Dummy { mac };
        assert_eq!(d.mac_address(), mac);
    }
}
