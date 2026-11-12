//! 下班提示音。规格 02 状态表：「到达下班时刻 → 弹『今日到账』通知 + 音效（可静音）」。
//!
//! ## 为什么是现场合成，不是打包一个 wav
//!
//! 一个几百毫秒的音频文件打进安装包，等于让这个「记工资的小工具」多背一份
//! 来源不明的二进制资源（版权、体积、审核上架时的额外说明）。而这两个音
//! 只差频率、包络和长度——二十行合成代码就够了。
//!
//! 更实际的原因：**音量**。播放 API 大多只能调「设备音量」，那会影响用户
//! 正在听的歌。自己合成的话，音量就是采样点上的一个乘数——百分之百可控、
//! 不碰系统任何全局状态。
//!
//! ## 为什么不用 WebView 里那个 `<audio>`
//!
//! 通知窗是隐藏的，被隐藏的 WebView 里放音在部分 WebView2 版本上会被
//! 自动播放策略挡掉；而且「响不响」这件事不该依赖一个正在降频的渲染进程。
//! 托盘看板那条线程（`spawn_beacon`）和播放同属系统层，两边对得上。
//!
//! ## 各平台怎么发声
//!
//! * Windows：`PlaySoundW(SND_MEMORY | SND_ASYNC)`——`winmm` 里唯一一个
//!   不需要 COM、不需要设备枚举、不需要消息泵的播放接口。
//! * macOS：写一份临时 wav 交给 `afplay`。不走 NSSound 是为了不引 objc 绑定
//!   （和 `platform.rs` 同一个取舍）。
//! * 其它平台：什么都不做。提示音没了不影响记账，日志里留一句。

use crate::settings::SoundTone;

/// 采样率。22.05 kHz 是「人耳够用 + 文件够小」的那个点：
/// 这两个音最高才 2 kHz，8 kHz 会有明显毛刺，44.1 kHz 纯属浪费。
const SAMPLE_RATE: u32 = 22_050;

/// 一个音符的合成参数。
struct Note {
    /// 基频（Hz）
    freq: f32,
    /// 时长（秒）
    dur: f32,
    /// 振幅（0–1）
    amp: f32,
    /// 起音时间（秒）。0 会让第一个采样点就是满幅，听感是「啪」的一声爆音。
    attack: f32,
    /// 衰减时间常数（秒）。越小越短促。
    tau: f32,
}

/// 「清脆」：两声上行（E6 → B6），短、亮，像微波炉「叮」。
const CRISP: [Note; 2] = [
    Note { freq: 1318.5, dur: 0.14, amp: 0.55, attack: 0.006, tau: 0.085 },
    Note { freq: 1975.5, dur: 0.34, amp: 0.48, attack: 0.006, tau: 0.130 },
];

/// 「轻柔」：单声 G5，慢衰减，尾巴长一点。
const SOFT: [Note; 1] = [Note {
    freq: 783.99,
    dur: 0.62,
    amp: 0.40,
    attack: 0.030,
    tau: 0.260,
}];

fn notes(tone: SoundTone) -> &'static [Note] {
    match tone {
        SoundTone::Crisp => &CRISP,
        SoundTone::Soft => &SOFT,
        // 静音不是「音量 0」：音量 0 还会走一遍播放路径（万一哪天加了
        // 播放前的副作用就会露出来），静音干脆什么都不生成。
        SoundTone::Silent => &[],
    }
}

/// 合成一段 wav。返回 `None` 表示「这次不该出声」。
///
/// 音量在这里就乘进采样点了：`volume` 是 0–100，超过 100 按 100 处理
/// （削顶比破音更难听）。`volume == 0` 返回 `None`，理由同上——
/// 没有声音的播放没有意义，还白白调一次系统 API。
pub fn wav(tone: SoundTone, volume: u32) -> Option<Vec<u8>> {
    let gain = volume.min(100) as f32 / 100.0;
    let list = notes(tone);
    if list.is_empty() || gain <= 0.0 {
        return None;
    }

    let total: usize = list
        .iter()
        .map(|n| (n.dur * SAMPLE_RATE as f32).round() as usize)
        .sum();
    if total == 0 {
        return None;
    }

    let mut pcm: Vec<i16> = Vec::with_capacity(total);
    for n in list {
        let len = (n.dur * SAMPLE_RATE as f32).round() as usize;
        for i in 0..len {
            let t = i as f32 / SAMPLE_RATE as f32;
            // 起音是一段线性爬坡，之后指数衰减。乘在一起就是「砰—嗡」。
            let atk = if n.attack <= 0.0 {
                1.0
            } else {
                (t / n.attack).min(1.0)
            };
            let env = atk * (-(t - n.attack).max(0.0) / n.tau).exp();
            // 加一点二次谐波：纯正弦听着像测试音，带一点泛音才像「提示音」。
            let s = (std::f32::consts::TAU * n.freq * t).sin()
                + 0.18 * (std::f32::consts::TAU * 2.0 * n.freq * t).sin();
            let v = n.amp * env * s / 1.18 * gain;
            // 再兜一道：参数写错也不该把 i16 的转换推出界。
            pcm.push((v.clamp(-1.0, 1.0) * i16::MAX as f32) as i16);
        }
    }

    Some(pack_wav(&pcm))
}

/// 给 16 位单声道 PCM 套一个标准 44 字节头。
fn pack_wav(pcm: &[i16]) -> Vec<u8> {
    let data_len = (pcm.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // fmt 块长度
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // 单声道
    out.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    out.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes()); // 字节率 = SR × 声道 × 位深/8
    out.extend_from_slice(&2u16.to_le_bytes()); // 块对齐
    out.extend_from_slice(&16u16.to_le_bytes()); // 位深
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in pcm {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// 放一次。失败不出声，也不影响任何别的功能。
pub fn play(tone: SoundTone, volume: u32) {
    let Some(bytes) = wav(tone, volume) else {
        return;
    };
    platform_play(bytes);
}

// ---------------------------------------------------------------- Windows

#[cfg(windows)]
fn platform_play(bytes: Vec<u8>) {
    use std::sync::Mutex;

    /// `SND_ASYNC` 意味着函数立刻返回、播放还在继续——那块内存必须一直有效
    /// 到播完为止。存在静态里是最省事的办法：下一次播放覆盖它，而
    /// `PlaySound` 本来就是「新的打断旧的」，覆盖的那一刻旧的已经不需要了。
    static BUF: Mutex<Vec<u8>> = Mutex::new(Vec::new());

    const SND_ASYNC: u32 = 0x0001;
    const SND_NODEFAULT: u32 = 0x0002;
    const SND_MEMORY: u32 = 0x0004;

    #[link(name = "winmm")]
    extern "system" {
        /// `pszSound` 在 `SND_MEMORY` 下是「指向 wav 镜像的指针」，
        /// 类型上写着 `LPCWSTR` —— 这是这个 API 历史遗留的怪处，
        /// 传字节指针才是对的。
        fn PlaySoundW(psz: *const u8, hmod: *mut std::ffi::c_void, flags: u32) -> i32;
    }

    let Ok(mut g) = BUF.lock() else {
        return;
    };
    *g = bytes;
    let ptr = g.as_ptr();
    // 不判返回值：放不出来（没有音频设备、被独占）不该写日志刷屏，
    // 也不该让「通知已经弹出来了」这件事跟着回滚。
    unsafe {
        PlaySoundW(ptr, std::ptr::null_mut(), SND_MEMORY | SND_ASYNC | SND_NODEFAULT);
    }
}

// ---------------------------------------------------------------- macOS

#[cfg(target_os = "macos")]
fn platform_play(bytes: Vec<u8>) {
    use std::io::Write;
    let path = std::env::temp_dir().join(format!("paypulse-chime-{}.wav", std::process::id()));
    let Ok(mut f) = std::fs::File::create(&path) else {
        crate::log::w("提示音：临时文件写不出来，这次不响");
        return;
    };
    if f.write_all(&bytes).is_err() {
        return;
    }
    let _ = f.flush();
    // `afplay` 是系统自带，不引依赖。音量已经烘进采样点了，不再传 -v。
    if let Err(e) = std::process::Command::new("afplay").arg(&path).spawn() {
        crate::log::w(&format!("提示音：afplay 起不来（{e}）"));
    }
}

// ---------------------------------------------------------------- 其它

#[cfg(not(any(windows, target_os = "macos")))]
fn platform_play(_bytes: Vec<u8>) {
    crate::log::w("提示音：这个平台没有播放实现，静默跳过");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 头部必须是播放器认得的那个形状。字段错一个，Windows 那边就是
    /// 「调用成功、一声不响」，而这条路上没有任何错误码可查。
    #[test]
    fn header_is_a_playable_wav() {
        let w = wav(SoundTone::Crisp, 100).expect("默认音量应该生成声音");
        assert_eq!(&w[0..4], b"RIFF");
        assert_eq!(&w[8..12], b"WAVE");
        assert_eq!(&w[12..16], b"fmt ");
        assert_eq!(&w[36..40], b"data");
        // RIFF 长度 = 文件长度 − 8
        let riff = u32::from_le_bytes([w[4], w[5], w[6], w[7]]) as usize;
        assert_eq!(riff, w.len() - 8);
        // data 长度 = 后面的字节数
        let data = u32::from_le_bytes([w[40], w[41], w[42], w[43]]) as usize;
        assert_eq!(data, w.len() - 44);
        // 采样率与位深
        assert_eq!(u32::from_le_bytes([w[24], w[25], w[26], w[27]]), SAMPLE_RATE);
        assert_eq!(u16::from_le_bytes([w[34], w[35]]), 16);
        // 两块音加起来约 0.48 秒，不该生成一段静默或者一段无限长
        assert!(data > 10_000 && data < 60_000, "data 长度不对：{data}");
    }

    /// 音量是乘数：一半音量 ≈ 一半峰值（一个采样点的量化误差以内）。
    #[test]
    fn volume_scales_the_waveform() {
        let peak = |v: u32| {
            let w = wav(SoundTone::Crisp, v).unwrap();
            w[44..]
                .chunks_exact(2)
                .map(|c| i16::from_le_bytes([c[0], c[1]]).unsigned_abs())
                .max()
                .unwrap()
        };
        let (full, half) = (peak(100), peak(50));
        assert!(full > 12_000, "满音量应该够响：{full}");
        assert!(
            (half as i32 - full as i32 / 2).abs() <= 1,
            "一半音量应该是一半峰值：{half} vs {full}"
        );
    }

    /// 音量为 0、选了静音：都不生成声音。
    /// 注意这是两条不同的路——静音连 wav 都不生成，音量为 0 是生成后丢弃，
    /// 两种都必须返回 `None`，否则「静音」会在某些实现对不上。
    #[test]
    fn zero_volume_and_silent_tone_make_no_sound() {
        assert!(wav(SoundTone::Crisp, 0).is_none());
        assert!(wav(SoundTone::Silent, 80).is_none());
        assert!(wav(SoundTone::Soft, 80).is_some());
    }

    /// 「轻柔」比「清脆」长，这是它们唯一能被听出来的结构性差别。
    #[test]
    fn soft_is_longer_than_crisp() {
        let c = wav(SoundTone::Crisp, 60).unwrap().len();
        let s = wav(SoundTone::Soft, 60).unwrap().len();
        assert!(s > c);
    }

    /// 越界音量被夹住，不该 panic、也不该因为溢出而变成噪音。
    #[test]
    fn absurd_volume_is_clamped_not_wrapped() {
        let a = wav(SoundTone::Crisp, 100).unwrap();
        let b = wav(SoundTone::Crisp, 65_535).unwrap();
        assert_eq!(a, b, "超过 100 应该按 100 算");
    }
}
