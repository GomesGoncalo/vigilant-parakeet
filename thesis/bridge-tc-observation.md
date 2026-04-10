Bridge + tc (netem) performance note

Methods — link emulation:
Network interfaces were attached to a Linux bridge and per-link impairment applied with tc/netem. Example command used:

    tc qdisc replace dev br0 root netem delay 50ms 10ms loss 1%

Batch application of qdisc rules was used to configure many links.

Measurements & results:
CPU usage was monitored interactively with htop. Even when applying tc rules in batches, the bridge+tc setup saturated the test machine (htop showed near‑100% CPU), preventing the system from scaling to the same node counts achievable with the project's in‑process channel simulation.

Interpretation & guidance:
This shows kernel‑level forwarding and shaping (bridge+tc) is substantially more CPU‑expensive than the simulator's in‑process channel approach. For thesis reporting include the exact tc parameters, machine specs (CPU model and core count), and a CPU% vs node count plot or small table to quantify the scalability gap.
