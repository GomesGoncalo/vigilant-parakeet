use web_sys::{Event, HtmlInputElement};
use yew::{function_component, html, use_node_ref, Callback, Html, Properties};

#[derive(Properties, PartialEq, Clone)]
pub struct Props {
    pub options: Vec<String>,
    pub onchange: Callback<String>,
}

#[function_component]
pub fn Select(props: &Props) -> Html {
    let div_ref = use_node_ref();
    let div_refc = div_ref.clone();
    let on_change_cb = props.onchange.clone();
    let onchange = {
        let input_node_ref = div_refc.clone();

        Callback::from(move |_: Event| {
            let input = input_node_ref.cast::<HtmlInputElement>();

            if let Some(input) = input {
                tracing::info!(value = input.value(), "asdasdsadas");
                on_change_cb.emit(input.value());
            }
        })
    };
    html! {
         <select ref={div_ref.clone()} onchange={onchange}>
        <option value="" selected=true disabled=true hidden=true>{"Choose node"}</option>
        {
            props.options.iter().map(|x| {
                yew::html! {
                    <option value={(*x).clone()}>{(*x).clone()}</option>
                }
            }).collect::<Html>()
        }
         </select>
    }
}
