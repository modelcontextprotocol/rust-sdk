use tokio_util::codec::Decoder;

#[derive(Default)]
pub struct JsonRpcFrameCodec;
impl Decoder for JsonRpcFrameCodec {
    type Item = tokio_util::bytes::Bytes;
    type Error = tokio::io::Error;
    fn decode(
        &mut self,
        src: &mut tokio_util::bytes::BytesMut,
    ) -> Result<Option<Self::Item>, Self::Error> {
        if let Some(end) = src
            .iter()
            .enumerate()
            .find_map(|(idx, &b)| (b == b'\n').then_some(idx))
        {
            let line = src.split_to(end);
            let _char_next_line = src.split_to(1);
            Ok(Some(line.freeze()))
        } else {
            Ok(None)
        }
    }
}
