// select and throughput live as separate files only in some branches; remove module decls here.
use common::channel_parameters::ChannelParameters;
use gloo_net::http::Request;
// Select component temporarily disabled; module not present in this branch.
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::collections::HashMap as StdHashMap;
use tracing_subscriber::fmt::format::Pretty;
use tracing_subscriber::prelude::*;
use tracing_web::{performance_layer, MakeWebConsoleWriter};
use web_sys::wasm_bindgen::JsCast;
use web_sys::HtmlInputElement;
use yew::prelude::*;
mod graph;
mod graph_helpers;
use crate::graph::Graph;

#[derive(Clone, PartialEq, Serialize, Deserialize, Debug)]
#[allow(dead_code)]
struct UpstreamInfo {
    hops: u32,
    mac: String,
    node_name: Option<String>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize, Debug)]
#[allow(dead_code)]
struct NodeInfo {
    node_type: String,
    upstream: Option<UpstreamInfo>,
    downstream: Option<Vec<UpstreamInfo>>,
}

#[derive(Clone, PartialEq, Properties)]
#[allow(dead_code)]
struct Props {
    nodes: Vec<String>,
    channels: HashMap<String, HashMap<String, ChannelParameters>>,
}

#[function_component]
fn OutterForm(props: &Props) -> Html {
    let from = use_state(|| None::<String>);
    let to = use_state(|| None::<String>);
    let fromc = from.clone();
    let toc = to.clone();
    let emit = Callback::from(move |(latency, loss)| {
        let fromc = fromc.clone();
        let toc = toc.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let (Some(_from), Some(_to)) = (&*fromc, &*toc) else {
                return;
            };

            tracing::debug!(latency, loss, "Channel parameters updated");
            //let map = HashMap::new();
            //let _ = Request::post(&format!("http://127.0.0.1:3030/channel/{}/{}", from, to).to_string()).json(format!("{{\"latency\":\"{latency}\",\"loss\":\"{loss}\"}}")).unwrap()
            //     .send()
            //     .await;
        });
    });

    let channels = props.channels.clone();
    let fromc = from.clone();
    // keep a handle to `to` for closures below
    let toc = to.clone();
    let emitc = emit.clone();
    let emit_latency = Callback::from(move |input_event: Event| {
        let input_event_target = input_event.target().unwrap();
        let current_input_text = input_event_target.unchecked_into::<HtmlInputElement>();

        let Ok(number) = current_input_text.value().parse() else {
            tracing::warn!("Failed to parse latency value");
            return;
        };

        if let (Some(from), Some(to)) = (&*fromc, &*toc) {
            tracing::debug!(latency_ms = number, "Latency updated");
            emitc.emit((number, channels.get(from).unwrap().get(to).unwrap().loss));
        }
    });

    let channels = props.channels.clone();
    let fromc = from.clone();
    let toc = to.clone();
    let emit_loss = Callback::from(move |input_event: Event| {
        let input_event_target = input_event.target().unwrap();
        let current_input_text = input_event_target.unchecked_into::<HtmlInputElement>();

        let Ok(number) = current_input_text.value().parse() else {
            tracing::warn!("Failed to parse loss rate value");
            return;
        };

        if let (Some(from), Some(to)) = (&*fromc, &*toc) {
            tracing::debug!(loss_rate = number, "Loss rate updated");
            emit.emit((
                channels
                    .get(from)
                    .unwrap()
                    .get(to)
                    .unwrap()
                    .latency
                    .as_millis(),
                number,
            ));
        }
    });

    html!(<>
        // <Select options={nodes.clone()} onchange={on_change} />
    // selection UI hidden in this branch
        {
            match (&*from, &*to) {
                (Some(from), Some(to)) => html! { <>
                        {" latency (ms): "}<input type="text" name={"latency"} value={props.channels.get(from).unwrap().get(to).unwrap().latency.as_millis().to_string()} onchange={emit_latency} />
                        {" loss (0.0-1.0): "}<input type="text" name={"loss"} value={props.channels.get(from).unwrap().get(to).unwrap().loss.to_string()} onchange={emit_loss} />
                    </>},
                _ => html! { { " Select options"} }
            }
        }
    </>)
}

#[function_component(App)]
fn app() -> Html {
    let nodes = use_state(Vec::default);
    let channels = use_state(HashMap::default);
    // node_info is stored as opaque JSON so the library code can treat it
    // uniformly when used as a dependency in tests.
    let node_info = use_state(StdHashMap::<String, JsonValue>::default);
    // stats: node -> (device_stats, tun_stats) as JSON value
    let stats = use_state(StdHashMap::<String, JsonValue>::default);
    {
        let nodes = nodes.clone();
        use_effect_with((), move |_| {
            let nodes = nodes.clone();
            wasm_bindgen_futures::spawn_local(async move {
                loop {
                    let Ok(request) = Request::get("http://127.0.0.1:3030/nodes").send().await
                    else {
                        gloo_timers::future::sleep(std::time::Duration::from_secs(1)).await;
                        continue;
                    };

                    if let Ok(mut node_stats) = request.json::<Vec<String>>().await {
                        node_stats.sort();
                        if *nodes != node_stats {
                            tracing::debug!(count = node_stats.len(), "Node list updated");
                            nodes.set(node_stats);
                        }
                    }

                    gloo_timers::future::sleep(std::time::Duration::from_secs(1)).await;
                }
            });
            || ()
        });
    }

    {
        let channels = channels.clone();
        use_effect_with((), move |_| {
            let channels = channels.clone();
            wasm_bindgen_futures::spawn_local(async move {
                loop {
                    let Ok(request) = Request::get("http://127.0.0.1:3030/channels").send().await
                    else {
                        gloo_timers::future::sleep(std::time::Duration::from_secs(1)).await;
                        continue;
                    };

                    if let Ok(channel_stats) = request
                        .json::<HashMap<String, HashMap<String, ChannelParameters>>>()
                        .await
                    {
                        if *channels != channel_stats {
                            let link_count: usize = channel_stats.values().map(|m| m.len()).sum();
                            tracing::debug!(link_count, "Channel stats updated");
                            channels.set(channel_stats);
                        }
                    }

                    gloo_timers::future::sleep(std::time::Duration::from_secs(1)).await;
                }
            });
            || ()
        });
    }

    // Poll node_info endpoint
    {
        let node_info = node_info.clone();
        use_effect_with((), move |_| {
            let node_info = node_info.clone();
            wasm_bindgen_futures::spawn_local(async move {
                loop {
                    let Ok(request) = Request::get("http://127.0.0.1:3030/node_info").send().await
                    else {
                        gloo_timers::future::sleep(std::time::Duration::from_secs(1)).await;
                        continue;
                    };

                    if let Ok(map) = request.json::<StdHashMap<String, JsonValue>>().await {
                        if *node_info != map {
                            node_info.set(map);
                        }
                    }

                    gloo_timers::future::sleep(std::time::Duration::from_secs(1)).await;
                }
            });
            || ()
        });
    }

    // Watch nodes, channels and node_info state and re-render the Graph component via Yew (props-driven)

    // Poll /stats endpoint (device + tun stats)
    {
        let stats = stats.clone();
        use_effect_with((), move |_| {
            let stats = stats.clone();
            wasm_bindgen_futures::spawn_local(async move {
                loop {
                    let Ok(request) = Request::get("http://127.0.0.1:3030/stats").send().await
                    else {
                        gloo_timers::future::sleep(std::time::Duration::from_secs(1)).await;
                        continue;
                    };

                    if let Ok(map) = request.json::<StdHashMap<String, JsonValue>>().await {
                        if *stats != map {
                            stats.set(map);
                        }
                    }

                    gloo_timers::future::sleep(std::time::Duration::from_secs(1)).await;
                }
            });
            || ()
        });
    }

    html! {
        <>
            <Graph nodes={(*nodes).clone()} channels={(*channels).clone()} node_info={(*node_info).clone()} stats={(*stats).clone()} />
        </>
    }
}

fn main() {
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_ansi(false) // Only partially supported across browsers
        .without_time() // std::time is not available in browsers, see note below
        .with_writer(MakeWebConsoleWriter::new()); // write events to the console
    let perf_layer = performance_layer().with_details_from_fields(Pretty::default());

    tracing_subscriber::registry()
        .with(fmt_layer)
        .with(perf_layer)
        .init(); // Install these as subscribers to tracing events
    yew::Renderer::<App>::new().render();
}
