# vigilant-parakeet

This is an implementation of [this paper](https://www.researchgate.net/publication/286923369_L3_Security_in_Vehicular_Networks).

It is still in progress and as such does not implement anything more than the routing.
One of the challenges I had when testing this was simulating different networks. Had to run around in the lab to simulate losses, and latency changes.

This repo addresses this from the start by creating a simulator.
Obviously it doesn't test all the parameters we'd be able to observe in a real environment, but I added latency and packet loss.

This is how to run this:
1. Checkout the repo
2. `cargo build --bin simulator --release --features webview`
3. Create configuration files.

There should be a config for the simulator and then configs for each node (this can probably also execute without the simulator but I don't have hardware to try it)

simulator config (put it in a yaml file) defines the topology and starting channels parameters:
```
nodes: 
  n1:
    config_path: n1.yaml
  n2:
    config_path: n2.yaml
topology:
  n1:
    n2:
      latency: 0
      loss: 0
  n2:
    n1:
      latency: 0
      loss: 0
  n3:
    n1:
      latency: 0
      loss: 0
```

Then create n1 config (in a yaml file please):
for the Rsu
```
node_type: Rsu
hello_history: 10
hello_periodicity: 5000
ip: 10.0.0.1
```

Then create n2 config (in a yaml file please):
for the Obu
```
node_type: Obu
hello_history: 10
ip: 10.0.0.2
```

then launch it:
```
❯ sudo RUST_LOG="node=debug" ./target/release/simulator --config-file file.yaml --pretty
  2024-03-18T17:52:24.046972Z  INFO node_lib::control::obu: Setup Obu, obu.args: Args { bind: "real", tap_name: Some("virtual"), ip: Some(10.0.0.3), mtu: 1459, node_params: NodeParameters { node_type: Obu, hello_history: 10, hello_periodicity: None } }
    at node_lib/src/control/obu/mod.rs:46 on ThreadId(9)

  2024-03-18T17:52:24.082809Z  INFO node_lib::control::rsu: Setup Rsu, rsu.args: Args { bind: "real", tap_name: Some("virtual"), ip: Some(10.0.0.1), mtu: 1459, node_params: NodeParameters { node_type: Rsu, hello_history: 10, hello_periodicity: Some(5000) } }
    at node_lib/src/control/rsu/mod.rs:48 on ThreadId(7)

  2024-03-18T17:52:24.082949Z DEBUG node_lib::control::obu::routing: route created on heartbeat, from: 3E:CD:17:95:5E:01, to: 66:1E:70:54:27:6A, through: Route { mac: 66:1E:70:54:27:6A, hops: 1, latency: None }
    at node_lib/src/control/obu/routing.rs:135 on ThreadId(3)

  2024-03-18T17:52:24.082996Z DEBUG node_lib::control::obu::routing: route created on heartbeat, from: 3A:0D:1E:BB:64:D0, to: 66:1E:70:54:27:6A, through: Route { mac: 66:1E:70:54:27:6A, hops: 1, latency: None }
    at node_lib/src/control/obu/routing.rs:135 on ThreadId(7)

  2024-03-18T17:52:24.083041Z DEBUG node_lib::control::rsu::routing: route created from heartbeat reply, from: 66:1E:70:54:27:6A, to: 3E:CD:17:95:5E:01, through: Route { mac: 3E:CD:17:95:5E:01, hops: 1, latency: Some(233µs) }
    at node_lib/src/control/rsu/routing.rs:121 on ThreadId(9)

  2024-03-18T17:52:24.083080Z DEBUG node_lib::control::rsu::routing: route created from heartbeat reply, from: 66:1E:70:54:27:6A, to: 3A:0D:1E:BB:64:D0, through: Route { mac: 3A:0D:1E:BB:64:D0, hops: 1, latency: Some(283µs) }
    at node_lib/src/control/rsu/routing.rs:121 on ThreadId(9)
```

Get traffic stats by using this:
```
❯ curl http://127.0.0.1:3030/stats | jq
{
  "n1": {
    "received_packets": 44,
    "received_bytes": 3074,
    "transmitted_packets": 33,
    "transmitted_bytes": 3210
  },
  "n2": {
    "received_packets": 33,
    "received_bytes": 2642,
    "transmitted_packets": 18,
    "transmitted_bytes": 1200
  },
  "n3": {
    "received_packets": 32,
    "received_bytes": 2528,
    "transmitted_packets": 19,
    "transmitted_bytes": 1308
  }
}
```

Change channel properties by using this:
```
❯ curl --header "Content-Type: application/json" \
  --request POST \
  --data '{"latency":"100","loss":"0.0"}' \
  http://localhost:3030/channel/n1/n2/
```

etc etc etc

You can use iperf:
```
❯ sudo ip netns exec sim_ns_n1 runuser -l $USER -c "iperf -s -i 1"
❯ sudo ip netns exec sim_ns_n2 runuser -l $USER -c "iperf -c 10.0.0.1 -i 1 -t 10000"
```

or ping:
```
❯ sudo ip netns exec sim_ns_n2 runuser -l $USER -c "ping 10.0.0.1"
PING 10.0.0.1 (10.0.0.1) 56(84) bytes of data.
64 bytes from 10.0.0.1: icmp_seq=1 ttl=64 time=0.530 ms
```
