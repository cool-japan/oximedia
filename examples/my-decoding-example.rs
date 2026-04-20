use oximedia_codec::{Av1Decoder, VideoDecoder};
use oximedia_codec::traits::DecoderConfig;
use oximedia_container::demux::{Demuxer, MatroskaDemuxer};
use oximedia_core::{CodecId, OxiError};
use oximedia_io::FileSource;

#[tokio::main]
async fn main() {
    let source = FileSource::open("fixtures/BigBuckBunny-AV1.webm")
        .await
        .unwrap();
    let mut demuxer = MatroskaDemuxer::new(source);
    demuxer.probe().await.unwrap();

    let video_stream = demuxer
        .streams()
        .iter()
        .find(|s| s.codec == CodecId::Av1)
        .expect("no AV1 video stream found");

    let video_stream_index = video_stream.index;

    // AV1CodecConfigurationRecord in WebM has a 4-byte header before the OBUs;
    // skip it so the decoder only sees raw OBUs. The sequence header also
    // appears in-stream so extradata is optional.
    let extradata = video_stream.codec_params.extradata.as_deref().and_then(|b| {
        if b.len() > 4 { Some(b[4..].to_vec()) } else { None }
    });
    let config = DecoderConfig {
        codec: CodecId::Av1,
        extradata,
        threads: 0,
        low_latency: false,
    };
    let mut decoder = Av1Decoder::new(config).unwrap();

    let mut decoded_frame = None;
    let mut video_packet_count = 0;
    let mut decode_errors = 0;

    loop {
        let packet = match demuxer.read_packet().await {
            Ok(p) => p,
            Err(OxiError::Eof) => break,
            Err(e) => panic!("demux error: {e}"),
        };

        if packet.stream_index != video_stream_index {
            continue;
        }

        if packet.data.is_empty() {
            continue;
        }

        video_packet_count += 1;

        if video_packet_count <= 3 {
            eprintln!(
                "packet {} size={} first_bytes={:02X?}",
                video_packet_count,
                packet.data.len(),
                &packet.data[..packet.data.len().min(8)]
            );
        }

        if let Err(e) = decoder.send_packet(&packet.data, packet.pts()) {
            decode_errors += 1;
            if decode_errors <= 3 {
                eprintln!("packet {} decode error: {e}", video_packet_count);
            }
            continue;
        }

        while let Ok(Some(frame)) = decoder.receive_frame() {
            if video_packet_count == 100 {
                decoded_frame = Some(frame);
            }
        }

        if video_packet_count >= 100 {
            break;
        }
    }

    println!(
        "Processed {} video packets ({} decode errors)",
        video_packet_count, decode_errors
    );

    let frame = match decoded_frame {
        Some(f) => f,
        None => {
            eprintln!("No frame decoded at packet 100 — decoder may still be in development");
            return;
        }
    };

    println!("Frame 100: {}x{}, format={:?}", frame.width, frame.height, frame.format);

    let w = frame.width as usize;
    let h = frame.height as usize;
    let y_plane = &frame.planes[0];
    let u_plane = &frame.planes[1];
    let v_plane = &frame.planes[2];

    let mut rgb = vec![0u8; w * h * 3];
    for row in 0..h {
        for col in 0..w {
            let y = y_plane.data[row * y_plane.stride + col] as f32;
            let u = u_plane.data[(row / 2) * u_plane.stride + (col / 2)] as f32 - 128.0;
            let v = v_plane.data[(row / 2) * v_plane.stride + (col / 2)] as f32 - 128.0;

            let r = (y + 1.402 * v).clamp(0.0, 255.0) as u8;
            let g = (y - 0.344_136 * u - 0.714_136 * v).clamp(0.0, 255.0) as u8;
            let b = (y + 1.772 * u).clamp(0.0, 255.0) as u8;

            let idx = (row * w + col) * 3;
            rgb[idx] = r;
            rgb[idx + 1] = g;
            rgb[idx + 2] = b;
        }
    }

    use std::io::Write;
    let mut file = std::fs::File::create("frame_100.ppm").unwrap();
    write!(file, "P6\n{w} {h}\n255\n").unwrap();
    file.write_all(&rgb).unwrap();
    println!("Written to frame_100.ppm");
}
