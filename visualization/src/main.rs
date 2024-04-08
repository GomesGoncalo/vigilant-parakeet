mod select;
mod throughput;
use common::channel_parameters::ChannelParameters;
use gloo_net::http::Request;
use select::Select;
use std::collections::HashMap;
use throughput::Throughput;
use tracing_subscriber::fmt::format::Pretty;
use tracing_subscriber::prelude::*;
use tracing_web::{performance_layer, MakeWebConsoleWriter};
use web_sys::wasm_bindgen::JsCast;
use web_sys::HtmlInputElement;
use yew::prelude::*;

#[derive(Clone, PartialEq, Properties)]
struct Props {
    nodes: Vec<String>,
    channels: HashMap<String, HashMap<String, ChannelParameters>>,
}

#[function_component]
fn OutterForm(props: &Props) -> Html {
    let remaining = use_state(|| None::<Vec<String>>);
    let from = use_state(|| None::<String>);
    let to = use_state(|| None::<String>);
    let remainingc = remaining.clone();
    let nodesc = props.nodes.clone();
    let fromc = from.clone();
    let on_change: Callback<String> = Callback::from(move |node: String| {
        remainingc.set(Some(
            nodesc.iter().filter(|x| **x != node).cloned().collect(),
        ));
        fromc.set(Some(node))
    });
    let fromc = from.clone();
    let toc = to.clone();
    let emit = Callback::from(move |(latency, loss)| {
        let fromc = fromc.clone();
        let toc = toc.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let (Some(from), Some(to)) = (&*fromc, &*toc) else {
                return;
            };

            //let map = HashMap::new();
            tracing::info!(latency, loss, "emitted params");
            //let _ = Request::post(&format!("http://127.0.0.1:3030/channel/{}/{}", from, to).to_string()).json(format!("{{\"latency\":\"{latency}\",\"loss\":\"{loss}\"}}")).unwrap()
            //     .send()
            //     .await;
         });
    });

    let channels = props.channels.clone();
    let fromc = from.clone();
    let toc = to.clone();
    let emitc = emit.clone();
    let emit_latency = Callback::from(move |input_event: Event| {
        let input_event_target = input_event.target().unwrap();
        let current_input_text = input_event_target.unchecked_into::<HtmlInputElement>();

        let Ok(number) = current_input_text.value().parse() else {
            tracing::info!("Could not parse loss");
            return;
        };

        if let (Some(from), Some(to)) = (&*fromc, &*toc) {
            tracing::info!(number, "emitted latency");
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
            tracing::info!("Could not parse loss");
            return;
        };

        if let (Some(from), Some(to)) = (&*fromc, &*toc) {
            tracing::info!(number, "emitted loss");
            emit.emit((channels.get(from).unwrap().get(to).unwrap().latency.as_millis(), number));
        }
    });

    let toc = to.clone();
    let on_change_to: Callback<String> = Callback::from(move |node: String| toc.set(Some(node)));
    let nodes = &props.nodes;
    html!(<>
        <Select options={nodes.clone()} onchange={on_change} />
        {
            match &*remaining {
                None => html! {  },
                Some(remaining) => html! {
                    <>
                    <Select options={remaining.to_vec()} onchange={on_change_to} />
                    </>
                },
            }
        }
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
    let nodes = use_state(|| Vec::default());
    let channels = use_state(|| HashMap::default());
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
                        tracing::info!(?node_stats, "these are the current nodes");
                        if *nodes != node_stats {
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
                        tracing::info!(?channel_stats, "these are the current stats");
                        if *channels != channel_stats {
                            channels.set(channel_stats);
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
            <OutterForm nodes={nodes.to_vec()} channels={(*channels).clone()}/>
            <Throughput nodes={nodes.to_vec()}/>
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
