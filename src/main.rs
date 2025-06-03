use dioxus::{
    logger::tracing::warn,
    prelude::{
        server_fn::codec::{StreamingText, TextStream},
        *,
    },
};

use futures::StreamExt as _;

const FAVICON: Asset = asset!("/assets/favicon.ico");
const MAIN_CSS: Asset = asset!("/assets/main.css");
//const HEADER_SVG: Asset = asset!("/assets/header.svg");
const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");

#[cfg(feature = "server")]
mod nats_utilities {
    use tokio::sync::OnceCell;

    static JETSTREAM: OnceCell<async_nats::jetstream::Context> = OnceCell::const_new();

    async fn jetstream() -> async_nats::jetstream::Context {
        let nats_client = async_nats::connect("nats://localhost:4222").await.unwrap();
        async_nats::jetstream::new(nats_client)
    }

    pub async fn get_jetstream() -> &'static async_nats::jetstream::Context {
        JETSTREAM.get_or_init(jetstream).await
    }
}

// Discussion about using toikio for async server functions:
// https://github.com/DioxusLabs/dioxus/discussions/2310

// State management:
// https://bcnrust.github.io/devbcn-workshop/frontend/03_04_state_management.html

fn main() {
    // Init logger at INFO level to suppress NATS DEBUG output
    dioxus_logger::init(dioxus_logger::tracing::Level::INFO).expect("failed to init logger");
    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    rsx! {
        document::Link { rel: "icon", href: FAVICON }
        document::Link { rel: "stylesheet", href: MAIN_CSS }
        document::Link { rel: "stylesheet", href: TAILWIND_CSS }
        MessageViewer {}
    }
}

#[component]
pub fn MessageViewer() -> Element {
    let mut msg =
        use_signal(|| String::from("Enter a subject and click \"Watch Subject\" to see messages."));
    let mut task_handle: Signal<Option<Task>> = use_signal(|| None);

    let submit_handler = move |evt: Event<FormData>| {
        let subject = evt.values()["subject"].as_value();
        if subject.is_empty() {
            return;
        }
        if let Some(existing_task) = *task_handle.read() {
            existing_task.cancel();
        }
        let task = spawn(async move {
            if let Ok(stream) = msg_stream(subject).await {
                let mut stream = stream.into_inner();
                while let Some(Ok(s)) = stream.next().await {
                    msg.set(s);
                }
            }
        });
        task_handle.set(Some(task));
    };

    rsx! {
        div { class: "container",
            header {
                h1 { "Live Message Display" }
            }
            main {
                section { class: "form-section",
                    h2 { "Message Selector" }
                    form { id: "messageSelectorForm", onsubmit: submit_handler,
                        div { class: "form-group",
                            label { r#for: "subject", "Subject:" }
                            input {
                                id: "subjectInput",
                                name: "subject",
                                placeholder: "e.g., alerts.critical, sensor.temp.*",
                                r#type: "text",
                            }
                        }
                        button { r#type: "submit", "Watch Subject" }
                    }
                }
                ReceivedMessages { msg }
            }
            footer {
                p { "© Various Robots 2025" }
            }
        }
    }
}

#[component]
pub fn SelectorForm() -> Element {
    rsx! {}
}

#[component]
pub fn ReceivedMessages(msg: String) -> Element {
    rsx! {
        section { class: "messages-section",
            h3 {
                "Received Messages (Subject: "
                span { id: "currentSubjectDisplay", "None" }
                ")"
            }
            div { class: "highlighted-area", id: "messageDisplayArea",
                p { class: "placeholder-message", {msg} }
            }
        }
    }
}

#[server(output = StreamingText)]
async fn msg_stream(subject: String) -> Result<TextStream, ServerFnError> {
    let jetstream = nats_utilities::get_jetstream().await;
    let kv = jetstream
        .create_key_value(async_nats::jetstream::kv::Config {
            bucket: "messages".to_string(),
            ..Default::default()
        })
        .await?;
    println!("watching subject: {}", &subject);
    let mut watch = kv.watch(subject).await?;
    let (tx, rx) = futures::channel::mpsc::unbounded();
    tokio::spawn(async move {
        while let Some(msg) = watch.next().await {
            match msg {
                Ok(entry) => {
                    if tx
                        .unbounded_send(Ok(std::str::from_utf8(&entry.value).unwrap().to_string()))
                        .is_err()
                    {
                        warn!(" msg_stream() : stream close");
                        tx.close_channel();
                        return;
                    }
                }
                Err(e) => {
                    eprintln!("Error receiving message: {}", e);
                }
            }
        }
    });

    Ok(TextStream::new(rx))
}
