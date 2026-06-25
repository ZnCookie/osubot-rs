use super::*;
use crate::onebot::send_group_msg_with_record;
use crate::send_error;

pub(super) async fn render_audio(
    ctx: &BotContext,
    msg: &QQMessage,
    resp_tx: &mpsc::Sender<String>,
    score: &Score,
    _mode: GameMode,
) {
    render_audio_by_beatmapset_id(ctx, msg, resp_tx, score.beatmapset_id).await;
}

pub(super) async fn render_audio_by_beatmapset_id(
    ctx: &BotContext,
    msg: &QQMessage,
    resp_tx: &mpsc::Sender<String>,
    beatmapset_id: i64,
) {
    let qq = msg.user_id;
    let group_id = msg.group_id;

    if beatmapset_id <= 0 {
        send_error(resp_tx, qq, "error.data_fetch_failed").await;
        return;
    }

    let mp3 = match api::download_beatmap_preview_mp3(beatmapset_id).await {
        Ok(bytes) => bytes,
        Err(e) => {
            let _ = resp_tx.send(api_error_msg(qq, &e)).await;
            return;
        }
    };

    let write = ctx.write.clone();
    if let Err(e) = send_group_msg_with_record(&write, group_id, &mp3).await {
        warn!(error = %e, "{}", log_fmt!("main.beatmap_audio_send_failed", error = &e.to_string()));
        let _ = resp_tx
            .send(user_str("error.audio_send_failed").replace("{qq}", &qq.to_string()))
            .await;
    }
}
