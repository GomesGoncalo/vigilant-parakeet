use chrono::{DateTime, Local};
use common::stats::Stats;
use gloo_net::http::Request;
use gloo_timers::callback::Timeout;
use std::collections::HashMap;
use yew::prelude::*;
use yew_plotly::{
    plotly::{common::Title, layout::Axis, Layout, Plot},
    Plotly,
};

#[derive(Clone, PartialEq, Properties)]
pub struct Props {
    pub nodes: Vec<String>,
}

enum PlotState {
    NoData,
    First((DateTime<Local>, HashMap<String, (Stats, Stats)>)),
    Diff(
        (
            (DateTime<Local>, HashMap<String, (Stats, Stats)>),
            (DateTime<Local>, HashMap<String, (Stats, Stats)>),
        ),
    ),
}

pub struct Throughput {
    plot_state: PlotState,
    timeout: Timeout,
}

pub enum Msg {
    None,
    Timer,
    Data(DateTime<Local>, HashMap<String, (Stats, Stats)>),
}

impl Component for Throughput {
    type Message = Msg;
    type Properties = Props;

    fn create(ctx: &Context<Self>) -> Self {
        let clock_handle = {
            let link = ctx.link().clone();
            Timeout::new(1, move || link.send_message(Msg::Timer))
        };
        Self {
            plot_state: PlotState::NoData,
            timeout: clock_handle,
        }
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            Msg::Timer => {
                ctx.link().send_future(async {
                    let Ok(request) = Request::get("http://127.0.0.1:3030/stats").send().await
                    else {
                        return Msg::None;
                    };
                    let Ok(node_stats): Result<HashMap<String, (Stats, Stats)>, _> =
                        request.json().await
                    else {
                        return Msg::None;
                    };
                    Msg::Data(Local::now(), node_stats)
                });
                false
            }
            Msg::Data(instant, data) => {
                self.timeout = {
                    let link = ctx.link().clone();
                    Timeout::new(1000, move || link.send_message(Msg::Timer))
                };
                match &self.plot_state {
                    PlotState::NoData => {
                        self.plot_state = PlotState::First((instant, data));
                        false
                    }
                    PlotState::First((si, sd)) | PlotState::Diff((_, (si, sd))) => {
                        self.plot_state = PlotState::Diff(((*si, sd.clone()), (instant, data)));
                        true
                    }
                }
            }
            Msg::None => {
                self.timeout = {
                    let link = ctx.link().clone();
                    Timeout::new(1000, move || link.send_message(Msg::Timer))
                };
                false
            }
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        match &self.plot_state {
            PlotState::NoData | PlotState::First(_) => html! {
                <p>{"No data to be shown"}</p>
            },
            PlotState::Diff(((p_i, previous), (l_i, last))) => {
                let mut bytes_plot = Plot::new();
                let mut bytes_plottun = Plot::new();
                let mut pkt_plot = Plot::new();
                let nodes = ctx.props().nodes.clone();
                let rx_diff: Vec<_> = nodes
                    .iter()
                    .map(|node| {
                        let Some((lstat, lstattun)) = last.get(node) else {
                            return (0, 0);
                        };
                        let Some((fstat, fstattun)) = previous.get(node) else {
                            return (0, 0);
                        };
                        (
                            lstat.received_bytes.saturating_sub(fstat.received_bytes) * 8,
                            lstattun
                                .received_bytes
                                .saturating_sub(fstattun.received_bytes)
                                * 8,
                        )
                    })
                    .collect();
                let tx_diff: Vec<_> = nodes
                    .iter()
                    .map(|node| {
                        let Some((lstat, lstattun)) = last.get(node) else {
                            return (0, 0);
                        };
                        let Some((fstat, fstattun)) = previous.get(node) else {
                            return (0, 0);
                        };
                        (
                            lstat
                                .transmitted_bytes
                                .saturating_sub(fstat.transmitted_bytes)
                                * 8,
                            lstattun
                                .transmitted_bytes
                                .saturating_sub(fstattun.transmitted_bytes)
                                * 8,
                        )
                    })
                    .collect();
                let rxp_diff: Vec<_> = nodes
                    .iter()
                    .map(|node| {
                        let Some((lstat, _)) = last.get(node) else {
                            return 0;
                        };
                        let Some((fstat, _)) = previous.get(node) else {
                            return 0;
                        };
                        lstat
                            .received_packets
                            .saturating_sub(fstat.received_packets)
                    })
                    .collect();
                let txp_diff: Vec<_> = nodes
                    .iter()
                    .map(|node| {
                        let Some((lstat, _)) = last.get(node) else {
                            return 0;
                        };
                        let Some((fstat, _)) = previous.get(node) else {
                            return 0;
                        };
                        lstat
                            .transmitted_packets
                            .saturating_sub(fstat.transmitted_packets)
                    })
                    .collect();
                tracing::info!(
                    ?nodes,
                    ?tx_diff,
                    ?rx_diff,
                    ?txp_diff,
                    ?rxp_diff,
                    time_diff = (*l_i - p_i).num_milliseconds(),
                    "plotting"
                );

                let rtrace = yew_plotly::plotly::Bar::new(
                    ctx.props().nodes.clone(),
                    rx_diff.iter().cloned().map(|(dev, _)| dev).collect(),
                )
                .name("Received");
                let ttrace = yew_plotly::plotly::Bar::new(
                    ctx.props().nodes.clone(),
                    tx_diff.iter().cloned().map(|(dev, _)| dev).collect(),
                )
                .name("Transmitted");
                bytes_plot.add_trace(rtrace);
                bytes_plot.add_trace(ttrace);
                let rtrace = yew_plotly::plotly::Bar::new(
                    ctx.props().nodes.clone(),
                    rx_diff.iter().cloned().map(|(_, tun)| tun).collect(),
                )
                .name("Received");
                let ttrace = yew_plotly::plotly::Bar::new(
                    ctx.props().nodes.clone(),
                    tx_diff.iter().cloned().map(|(_, tun)| tun).collect(),
                )
                .name("Transmitted");
                bytes_plottun.add_trace(rtrace);
                bytes_plottun.add_trace(ttrace);
                let x_axis = Axis::new().title(Title::new("Nodes"));
                let y_axis = Axis::new().title(Title::new("Bits per second"));
                let layout = Layout::new()
                    .title("<b>Traffic</b>".into())
                    .x_axis(x_axis.clone())
                    .y_axis(y_axis.clone());
                bytes_plot.set_layout(layout);
                let layout = Layout::new()
                    .title("<b>Traffic (Tun)</b>".into())
                    .x_axis(x_axis.clone())
                    .y_axis(y_axis);
                bytes_plottun.set_layout(layout);
                let rtrace = yew_plotly::plotly::Bar::new(ctx.props().nodes.clone(), rxp_diff)
                    .name("Received");
                let ttrace = yew_plotly::plotly::Bar::new(ctx.props().nodes.clone(), txp_diff)
                    .name("Transmitted");
                pkt_plot.add_trace(rtrace);
                pkt_plot.add_trace(ttrace);
                let y_axis = Axis::new().title(Title::new("Packets per second"));
                let layout = Layout::new()
                    .title("<b>Packets</b>".into())
                    .x_axis(x_axis)
                    .y_axis(y_axis);
                pkt_plot.set_layout(layout);
                html! {
                <>
                    <Plotly plot={bytes_plot}/>
                    <Plotly plot={bytes_plottun}/>
                    <Plotly plot={pkt_plot}/>
                </>
                }
            }
        }
    }
}
