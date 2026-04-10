Implementation — alternatives tried

Kernel bridge + tc (netem)

As an alternative to the project's in-process channel simulation, network interfaces were attached to a Linux bridge and per-link impairments applied with tc/netem. Example command used for each link:

    tc qdisc replace dev br0 root netem delay 50ms 10ms loss 1%

Batch application of qdisc rules was used to configure many links. CPU usage was monitored interactively with htop. Even when rules were applied in batches, the bridge+tc setup saturated the test machine (htop reported near‑100% CPU), preventing the system from scaling to the same node counts achieved by the in‑process simulator.

Interpretation

Kernel‑level forwarding and netem-based shaping impose a high per‑packet processing cost on the host. In this project the in‑process channel simulation (implemented in Rust within the node/simulator runtime) provided equivalent latency/jitter/loss semantics while using far less CPU, allowing significantly larger simulated topologies on the same hardware.

Reporting guidance

When including this in the thesis, report the exact tc parameters, the test machine specifications (CPU model and core count, RAM), and a small CPU% vs node count plot or table to quantify the scalability gap. Also note measurement method (htop) and that even with batch rule application the kernel approach exhausted available CPU.
