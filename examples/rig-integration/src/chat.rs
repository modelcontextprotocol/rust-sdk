use futures::StreamExt;
use rig::{
    agent::Agent,
    completion::{AssistantContent, CompletionModel},
    message::Message,
    streaming::StreamingChat,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};

pub async fn cli_chatbot<M>(chatbot: Agent<M>) -> anyhow::Result<()>
where
    M: CompletionModel,
{
    let mut chat_log = vec![];

    let mut output = BufWriter::new(tokio::io::stdout());
    let mut input = BufReader::new(tokio::io::stdin());
    output.write_all(b"Enter :q to quit\n").await?;
    loop {
        output.write_all(b"\x1b[32muser>\x1b[0m ").await?;
        // Flush stdout to ensure the prompt appears before input
        output.flush().await?;
        let mut input_buf = String::new();
        input.read_line(&mut input_buf).await?;
        // Remove the newline character from the input
        let input = input_buf.trim();
        // Check for a command to exit
        if input == ":q" {
            break;
        }
        match chatbot.stream_chat(input, chat_log.clone()).await {
            Ok(mut response) => {
                tracing::info!(%input);
                chat_log.push(Message::user(input));
                stream_output_agent_start(&mut output).await?;
                let mut message_buf = String::new();
                while let Some(message) = response.next().await {
                    match message {
                        Ok(AssistantContent::Text(text)) => {
                            message_buf.push_str(&text.text);
                            output_agent(text.text, &mut output).await?;
                        }
                        Ok(AssistantContent::ToolCall(tool_call)) => {
                            let name = tool_call.function.name;
                            let arguments = tool_call.function.arguments;
                            chat_log.push(Message::assistant(format!(
                                "Calling tool: {name} with args: {arguments}"
                            )));
                            let result = chatbot.tools.call(&name, arguments.to_string()).await;
                            match result {
                                Ok(tool_call_result) => {
                                    stream_output_agent_finished(&mut output).await?;
                                    stream_output_toolcall(&tool_call_result, &mut output).await?;
                                    stream_output_agent_start(&mut output).await?;
                                    chat_log.push(Message::user(tool_call_result));
                                }
                                Err(e) => {
                                    output_error(e, &mut output).await?;
                                }
                            }
                        }
                        Err(error) => {
                            output_error(error, &mut output).await?;
                        }
                    }
                }
                chat_log.push(Message::assistant(message_buf));
                stream_output_agent_finished(&mut output).await?;
            }
            Err(error) => {
                output_error(error, &mut output).await?;
            }
        }
    }

    Ok(())
}

pub async fn output_error(
    e: impl std::fmt::Display,
    output: &mut BufWriter<tokio::io::Stdout>,
) -> std::io::Result<()> {
    output
        .write_all(b"\x1b[1;31m\xE2\x9D\x8C ERROR: \x1b[0m")
        .await?;
    output.write_all(e.to_string().as_bytes()).await?;
    output.write_all(b"\n").await?;
    output.flush().await?;
    Ok(())
}

pub async fn output_agent(
    content: impl std::fmt::Display,
    output: &mut BufWriter<tokio::io::Stdout>,
) -> std::io::Result<()> {
    output.write_all(content.to_string().as_bytes()).await?;
    output.flush().await?;
    Ok(())
}

pub async fn stream_output_toolcall(
    content: impl std::fmt::Display,
    output: &mut BufWriter<tokio::io::Stdout>,
) -> std::io::Result<()> {
    output
        .write_all(b"\x1b[1;33m\xF0\x9F\x9B\xA0 Tool Call: \x1b[0m")
        .await?;
    output.write_all(content.to_string().as_bytes()).await?;
    output.write_all(b"\n").await?;
    output.flush().await?;
    Ok(())
}

pub async fn stream_output_agent_start(
    output: &mut BufWriter<tokio::io::Stdout>,
) -> std::io::Result<()> {
    output
        .write_all(b"\x1b[1;34m\xF0\x9F\xA4\x96 Agent: \x1b[0m")
        .await?;
    output.flush().await?;
    Ok(())
}

pub async fn stream_output_agent_finished(
    output: &mut BufWriter<tokio::io::Stdout>,
) -> std::io::Result<()> {
    output.write_all(b"\n").await?;
    output.flush().await?;
    Ok(())
}
