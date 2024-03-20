use mac_address::MacAddress;

pub trait NetworkInterface {
    fn mac_address(&self) -> MacAddress;
}
