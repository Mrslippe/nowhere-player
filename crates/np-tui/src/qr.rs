/*
 * QR Code generator library (Rust)
 *
 * Copyright (c) Project Nayuki. (MIT License)
 * https://www.nayuki.io/page/qr-code-generator-library
 *
 * Permission is hereby granted, free of charge, to any person obtaining a copy of
 * this software and associated documentation files (the "Software"), to deal in
 * the Software without restriction, including without limitation the rights to
 * use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of
 * the Software, and to permit persons to whom the Software is furnished to do so,
 * subject to the following conditions:
 * - The above copyright notice and this permission notice shall be included in
 *   all copies or substantial portions of the Software.
 * - The Software is provided "as is", without warranty of any kind, express or
 *   implied, including but not limited to the warranties of merchantability,
 *   fitness for a particular purpose and noninfringement. In no event shall the
 *   authors or copyright holders be liable for any claim, damages or other
 *   liability, whether in an action of contract, tort or otherwise, arising from,
 *   out of or in connection with the Software or the use or other dealings in the
 *   Software.
 */

//! 终端二维码渲染（T-0092b）。
//!
//! 编码核心 vendored 自 Project Nayuki 的 QR Code generator（MIT），只保留
//! byte 模式生成路径。渲染使用 Unicode 半块：
//!
//! ```text
//! 1 个模块宽 × 2 个模块高 → 1 个字符（▀ / ▄ / █ / 空格）
//! ```
//!
//! 每行包含左右安静区（各 [`QUIET_ZONE_MODULES`] 个模块），不会输出 ANSI 颜色，
//! 也不打印二维码承载的内容。
//!
//! 模块内部不联网、不生成图片、不打开浏览器，不新增依赖。

use qrcodegen::{QrCode, QrCodeEcc};

/// 左右/上下安静区宽度（模块）。标准建议 4，但 72x24 终端放不下完整 4 模块白边，
/// 这里压缩到 2；弹窗接入方仍可在渲染结果外再补空白。
const QUIET_ZONE_MODULES: usize = 2;

/// 生成终端二维码行。
///
/// 输入按 UTF-8 字节模式编码（二维码规范里的 byte mode），适合 URL 等 ASCII/UTF-8
/// 混合内容。返回的每个 `String` 是一行终端文本，宽度一致；行内只包含
/// `▀`、`▄`、`█`、空格。
///
/// 空输入与超出 QR 容量的输入返回 `Err`；错误信息不包含输入内容。
// T-0092b 只交付渲染模块，登录弹窗由后续卡片接入；接入后删除这个 allow。
pub(crate) fn render_qr(text: &str) -> Result<Vec<String>, String> {
    if text.is_empty() {
        return Err("二维码内容为空".to_owned());
    }

    // 登录 URL 优先保证容量，使用 Low；错误信息不回显内容。
    let qr = QrCode::encode_binary(text.as_bytes(), QrCodeEcc::Low)
        .map_err(|_| "二维码内容超出容量".to_owned())?;
    let size = usize::try_from(qr.size()).map_err(|_| "二维码尺寸非法".to_owned())?;
    let total = size + QUIET_ZONE_MODULES * 2;

    let mut lines = Vec::with_capacity(total.div_ceil(2));
    for y in (0..total).step_by(2) {
        let mut line = String::with_capacity(total * 3);
        for x in 0..total {
            let top = module_at(&qr, x, y, size);
            let bottom = y + 1 < total && module_at(&qr, x, y + 1, size);
            line.push(match (top, bottom) {
                (true, true) => '█',
                (true, false) => '▀',
                (false, true) => '▄',
                (false, false) => ' ',
            });
        }
        lines.push(line);
    }
    Ok(lines)
}

/// 读取含安静区坐标 `(x, y)` 处的模块；安静区与外边界一律返回浅色。
fn module_at(qr: &QrCode, x: usize, y: usize, size: usize) -> bool {
    if x < QUIET_ZONE_MODULES || y < QUIET_ZONE_MODULES {
        return false;
    }
    let x = x - QUIET_ZONE_MODULES;
    let y = y - QUIET_ZONE_MODULES;
    if x >= size || y >= size {
        return false;
    }
    // size 最大 177，转 i32 不会溢出。
    qr.get_module(x as i32, y as i32)
}

/// Project Nayuki `qrcodegen` 的精简 vendored 版本，仅保留生成二维码所需的代码。
///
/// 原始项目：<https://github.com/nayuki/QR-Code-generator>
/// 许可：MIT（见文件顶部版权与许可声明）。
///
/// 与原实现的差异：删除文本/数字/字母数字/ECI 分段、SVG 输出与公开 getter；
/// 生成算法（版本选择、Reed-Solomon、掩码评分）保持不变。
mod qrcodegen {

    /*---- QrCode functionality ----*/

    pub struct QrCode {
        version: Version,
        size: i32,
        errorcorrectionlevel: QrCodeEcc,
        modules: Vec<bool>,
        isfunction: Vec<bool>,
    }

    impl QrCode {
        /// 用 byte 模式编码给定二进制数据，自动选择最小版本。
        pub fn encode_binary(data: &[u8], ecl: QrCodeEcc) -> Result<Self, DataTooLong> {
            let segs = [QrSegment::make_bytes(data)];
            QrCode::encode_segments(&segs, ecl)
        }

        fn encode_segments(segs: &[QrSegment], ecl: QrCodeEcc) -> Result<Self, DataTooLong> {
            QrCode::encode_segments_advanced(segs, ecl, Version::MIN, Version::MAX, None, true)
        }

        fn encode_segments_advanced(
            segs: &[QrSegment],
            mut ecl: QrCodeEcc,
            minversion: Version,
            maxversion: Version,
            mask: Option<Mask>,
            boostecl: bool,
        ) -> Result<Self, DataTooLong> {
            assert!(minversion <= maxversion, "Invalid value");

            // 找到能容纳数据的最小版本。
            let mut version: Version = minversion;
            let datausedbits: usize = loop {
                let datacapacitybits: usize = QrCode::get_num_data_codewords(version, ecl) * 8;
                match QrSegment::get_total_bits(segs, version) {
                    Some(n) if n <= datacapacitybits => break n,
                    Some(n) if version >= maxversion => {
                        return Err(DataTooLong::DataOverCapacity(n, datacapacitybits));
                    }
                    None if version >= maxversion => return Err(DataTooLong::SegmentTooLong),
                    _ => version = Version::new(version.value() + 1),
                }
            };

            // 在不增大版本的前提下尽量提高纠错级别。
            for &newecl in &[QrCodeEcc::Medium, QrCodeEcc::Quartile, QrCodeEcc::High] {
                if boostecl && datausedbits <= QrCode::get_num_data_codewords(version, newecl) * 8 {
                    ecl = newecl;
                }
            }

            // 拼接所有分段的数据位。
            let mut bb = BitBuffer(Vec::new());
            for seg in segs {
                bb.append_bits(seg.mode.mode_bits(), 4);
                bb.append_bits(
                    u32::try_from(seg.numchars).unwrap(),
                    seg.mode.num_char_count_bits(version),
                );
                bb.0.extend_from_slice(&seg.data);
            }
            debug_assert_eq!(bb.0.len(), datausedbits);

            // 终止符，并按字节对齐。
            let datacapacitybits: usize = QrCode::get_num_data_codewords(version, ecl) * 8;
            debug_assert!(bb.0.len() <= datacapacitybits);
            let numzerobits: usize = std::cmp::min(4, datacapacitybits - bb.0.len());
            bb.append_bits(0, u8::try_from(numzerobits).unwrap());
            let numzerobits: usize = bb.0.len().wrapping_neg() & 7;
            bb.append_bits(0, u8::try_from(numzerobits).unwrap());
            debug_assert_eq!(bb.0.len() % 8, 0);

            // 交替填充 0xEC / 0x11，直到数据容量用尽。
            for padbyte in [0xECu8, 0x11].into_iter().cycle() {
                if bb.0.len() >= datacapacitybits {
                    break;
                }
                bb.append_bits(u32::from(padbyte), 8);
            }

            // 按大端把位打包成字节。
            let mut datacodewords = vec![0u8; bb.0.len() / 8];
            for (i, &bit) in bb.0.iter().enumerate() {
                datacodewords[i >> 3] |= u8::from(bit) << (7 - (i & 7));
            }

            Ok(QrCode::encode_codewords(version, ecl, &datacodewords, mask))
        }

        fn encode_codewords(
            ver: Version,
            ecl: QrCodeEcc,
            datacodewords: &[u8],
            mut msk: Option<Mask>,
        ) -> Self {
            let size = usize::from(ver.value()) * 4 + 17;
            let mut result = Self {
                version: ver,
                size: size as i32,
                errorcorrectionlevel: ecl,
                modules: vec![false; size * size],
                isfunction: vec![false; size * size],
            };

            result.draw_function_patterns();
            let allcodewords = result.add_ecc_and_interleave(datacodewords);
            result.draw_codewords(&allcodewords);

            // 自动选择罚分最低的掩码。
            if msk.is_none() {
                let mut minpenalty = i32::MAX;
                for i in 0u8..8 {
                    let i = Mask::new(i);
                    result.apply_mask(i);
                    result.draw_format_bits(i);
                    let penalty = result.get_penalty_score();
                    if penalty < minpenalty {
                        msk = Some(i);
                        minpenalty = penalty;
                    }
                    result.apply_mask(i); // XOR 撤销
                }
            }
            let msk = msk.unwrap();
            result.apply_mask(msk);
            result.draw_format_bits(msk);

            result.isfunction.clear();
            result.isfunction.shrink_to_fit();
            result
        }

        /*---- 公开读取 ----*/

        pub fn size(&self) -> i32 {
            self.size
        }

        pub fn get_module(&self, x: i32, y: i32) -> bool {
            (0..self.size).contains(&x) && (0..self.size).contains(&y) && self.module(x, y)
        }

        fn module(&self, x: i32, y: i32) -> bool {
            self.modules[(y * self.size + x) as usize]
        }

        fn module_mut(&mut self, x: i32, y: i32) -> &mut bool {
            &mut self.modules[(y * self.size + x) as usize]
        }

        /*---- 绘制功能图形 ----*/

        fn draw_function_patterns(&mut self) {
            // 水平/垂直定时图形
            let size = self.size;
            for i in 0..size {
                self.set_function_module(6, i, i % 2 == 0);
                self.set_function_module(i, 6, i % 2 == 0);
            }

            // 三个定位图形（会覆盖部分定时模块）
            self.draw_finder_pattern(3, 3);
            self.draw_finder_pattern(size - 4, 3);
            self.draw_finder_pattern(3, size - 4);

            // 校正图形
            let alignpatpos = self.get_alignment_pattern_positions();
            let numalign = alignpatpos.len();
            for i in 0..numalign {
                for j in 0..numalign {
                    // 跳过三个定位图形所在角
                    if !(i == 0 && j == 0
                        || i == 0 && j == numalign - 1
                        || i == numalign - 1 && j == 0)
                    {
                        self.draw_alignment_pattern(alignpatpos[i], alignpatpos[j]);
                    }
                }
            }

            // 格式信息（先画占位，构造器最后会重画）与版本信息
            self.draw_format_bits(Mask::new(0));
            self.draw_version();
        }

        fn draw_format_bits(&mut self, mask: Mask) {
            let bits: u32 = {
                let data = u32::from(self.errorcorrectionlevel.format_bits() << 3 | mask.value());
                let mut rem = data;
                for _ in 0..10 {
                    rem = (rem << 1) ^ ((rem >> 9) * 0x537);
                }
                (data << 10 | rem) ^ 0x5412
            };
            debug_assert_eq!(bits >> 15, 0);

            // 第一份
            for i in 0..6 {
                self.set_function_module(8, i, get_bit(bits, i));
            }
            self.set_function_module(8, 7, get_bit(bits, 6));
            self.set_function_module(8, 8, get_bit(bits, 7));
            self.set_function_module(7, 8, get_bit(bits, 8));
            for i in 9..15 {
                self.set_function_module(14 - i, 8, get_bit(bits, i));
            }

            // 第二份
            let size = self.size;
            for i in 0..8 {
                self.set_function_module(size - 1 - i, 8, get_bit(bits, i));
            }
            for i in 8..15 {
                self.set_function_module(8, size - 15 + i, get_bit(bits, i));
            }
            self.set_function_module(8, size - 8, true);
        }

        fn draw_version(&mut self) {
            if self.version.value() < 7 {
                return;
            }

            let bits: u32 = {
                let data = u32::from(self.version.value());
                let mut rem = data;
                for _ in 0..12 {
                    rem = (rem << 1) ^ ((rem >> 11) * 0x1F25);
                }
                data << 12 | rem
            };
            debug_assert_eq!(bits >> 18, 0);

            for i in 0..18 {
                let bit = get_bit(bits, i);
                let a = self.size - 11 + i % 3;
                let b = i / 3;
                self.set_function_module(a, b, bit);
                self.set_function_module(b, a, bit);
            }
        }

        fn draw_finder_pattern(&mut self, x: i32, y: i32) {
            for dy in -4..=4 {
                for dx in -4..=4 {
                    let xx = x + dx;
                    let yy = y + dy;
                    if (0..self.size).contains(&xx) && (0..self.size).contains(&yy) {
                        let dist = std::cmp::max(dx.abs(), dy.abs());
                        self.set_function_module(xx, yy, dist != 2 && dist != 4);
                    }
                }
            }
        }

        fn draw_alignment_pattern(&mut self, x: i32, y: i32) {
            for dy in -2..=2 {
                for dx in -2..=2 {
                    self.set_function_module(
                        x + dx,
                        y + dy,
                        std::cmp::max(dx.abs(), dy.abs()) != 1,
                    );
                }
            }
        }

        fn set_function_module(&mut self, x: i32, y: i32, isdark: bool) {
            *self.module_mut(x, y) = isdark;
            self.isfunction[(y * self.size + x) as usize] = true;
        }

        /*---- 码字与掩码 ----*/

        fn add_ecc_and_interleave(&self, data: &[u8]) -> Vec<u8> {
            let ver = self.version;
            let ecl = self.errorcorrectionlevel;
            assert_eq!(
                data.len(),
                QrCode::get_num_data_codewords(ver, ecl),
                "Illegal argument"
            );

            let numblocks = QrCode::table_get(&NUM_ERROR_CORRECTION_BLOCKS, ver, ecl);
            let blockecclen = QrCode::table_get(&ECC_CODEWORDS_PER_BLOCK, ver, ecl);
            let rawcodewords = QrCode::get_num_raw_data_modules(ver) / 8;
            let numshortblocks = numblocks - rawcodewords % numblocks;
            let shortblocklen = rawcodewords / numblocks;

            let mut blocks = Vec::<Vec<u8>>::with_capacity(numblocks);
            let rsdiv = QrCode::reed_solomon_compute_divisor(blockecclen);
            let mut k = 0;
            for i in 0..numblocks {
                let datlen = shortblocklen - blockecclen + usize::from(i >= numshortblocks);
                let mut dat = data[k..k + datlen].to_vec();
                k += datlen;
                let ecc = QrCode::reed_solomon_compute_remainder(&dat, &rsdiv);
                if i < numshortblocks {
                    dat.push(0);
                }
                dat.extend_from_slice(&ecc);
                blocks.push(dat);
            }

            let mut result = Vec::<u8>::with_capacity(rawcodewords);
            for i in 0..=shortblocklen {
                for (j, block) in blocks.iter().enumerate() {
                    // 短块补的 0 不参与交错
                    if i != shortblocklen - blockecclen || j >= numshortblocks {
                        result.push(block[i]);
                    }
                }
            }
            result
        }

        fn draw_codewords(&mut self, data: &[u8]) {
            assert_eq!(
                data.len(),
                QrCode::get_num_raw_data_modules(self.version) / 8,
                "Illegal argument"
            );

            let mut i = 0;
            let mut right = self.size - 1;
            while right >= 1 {
                if right == 6 {
                    right = 5;
                }
                for vert in 0..self.size {
                    for j in 0..2 {
                        let x = right - j;
                        let upward = (right + 1) & 2 == 0;
                        let y = if upward { self.size - 1 - vert } else { vert };
                        if !self.isfunction[(y * self.size + x) as usize] && i < data.len() * 8 {
                            *self.module_mut(x, y) =
                                get_bit(u32::from(data[i >> 3]), 7 - ((i as i32) & 7));
                            i += 1;
                        }
                    }
                }
                right -= 2;
            }
            debug_assert_eq!(i, data.len() * 8);
        }

        fn apply_mask(&mut self, mask: Mask) {
            for y in 0..self.size {
                for x in 0..self.size {
                    let invert = match mask.value() {
                        0 => (x + y) % 2 == 0,
                        1 => y % 2 == 0,
                        2 => x % 3 == 0,
                        3 => (x + y) % 3 == 0,
                        4 => (x / 3 + y / 2) % 2 == 0,
                        5 => x * y % 2 + x * y % 3 == 0,
                        6 => (x * y % 2 + x * y % 3) % 2 == 0,
                        7 => ((x + y) % 2 + x * y % 3) % 2 == 0,
                        _ => unreachable!(),
                    };
                    *self.module_mut(x, y) ^=
                        invert & !self.isfunction[(y * self.size + x) as usize];
                }
            }
        }

        fn get_penalty_score(&self) -> i32 {
            let mut result = 0;
            let size = self.size;

            // 行：连续同色与定位图形样式
            for y in 0..size {
                let mut runcolor = false;
                let mut runx = 0;
                let mut runhistory = FinderPenalty::new(size);
                for x in 0..size {
                    if self.module(x, y) == runcolor {
                        runx += 1;
                        if runx == 5 {
                            result += PENALTY_N1;
                        } else if runx > 5 {
                            result += 1;
                        }
                    } else {
                        runhistory.add_history(runx);
                        if !runcolor {
                            result += runhistory.count_patterns() * PENALTY_N3;
                        }
                        runcolor = self.module(x, y);
                        runx = 1;
                    }
                }
                result += runhistory.terminate_and_count(runcolor, runx) * PENALTY_N3;
            }

            // 列：连续同色与定位图形样式
            for x in 0..size {
                let mut runcolor = false;
                let mut runy = 0;
                let mut runhistory = FinderPenalty::new(size);
                for y in 0..size {
                    if self.module(x, y) == runcolor {
                        runy += 1;
                        if runy == 5 {
                            result += PENALTY_N1;
                        } else if runy > 5 {
                            result += 1;
                        }
                    } else {
                        runhistory.add_history(runy);
                        if !runcolor {
                            result += runhistory.count_patterns() * PENALTY_N3;
                        }
                        runcolor = self.module(x, y);
                        runy = 1;
                    }
                }
                result += runhistory.terminate_and_count(runcolor, runy) * PENALTY_N3;
            }

            // 2x2 同色块
            for y in 0..size - 1 {
                for x in 0..size - 1 {
                    let color = self.module(x, y);
                    if color == self.module(x + 1, y)
                        && color == self.module(x, y + 1)
                        && color == self.module(x + 1, y + 1)
                    {
                        result += PENALTY_N2;
                    }
                }
            }

            // 深浅比例
            let dark: i32 = self.modules.iter().copied().map(i32::from).sum();
            let total = size * size;
            let k = ((dark * 20 - total * 10).abs() + total - 1) / total - 1;
            debug_assert!((0..=9).contains(&k));
            result += k * PENALTY_N4;
            debug_assert!((0..=2568888).contains(&result));
            result
        }

        /*---- 私有辅助函数 ----*/

        fn get_alignment_pattern_positions(&self) -> Vec<i32> {
            let ver = i32::from(self.version.value());
            if ver == 1 {
                vec![]
            } else {
                let numalign = ver / 7 + 2;
                let step = (ver * 8 + numalign * 3 + 5) / (numalign * 4 - 4) * 2;
                let mut result: Vec<i32> = (0..numalign - 1)
                    .map(|i| self.size - 7 - i * step)
                    .collect();
                result.push(6);
                result.reverse();
                result
            }
        }

        fn get_num_raw_data_modules(ver: Version) -> usize {
            let ver = usize::from(ver.value());
            let mut result = (16 * ver + 128) * ver + 64;
            if ver >= 2 {
                let numalign = ver / 7 + 2;
                result -= (25 * numalign - 10) * numalign - 55;
                if ver >= 7 {
                    result -= 36;
                }
            }
            debug_assert!((208..=29648).contains(&result));
            result
        }

        fn get_num_data_codewords(ver: Version, ecl: QrCodeEcc) -> usize {
            QrCode::get_num_raw_data_modules(ver) / 8
                - QrCode::table_get(&ECC_CODEWORDS_PER_BLOCK, ver, ecl)
                    * QrCode::table_get(&NUM_ERROR_CORRECTION_BLOCKS, ver, ecl)
        }

        fn table_get(table: &'static [[i8; 41]; 4], ver: Version, ecl: QrCodeEcc) -> usize {
            table[ecl.ordinal()][usize::from(ver.value())] as usize
        }

        fn reed_solomon_compute_divisor(degree: usize) -> Vec<u8> {
            assert!((1..=255).contains(&degree), "Degree out of range");
            let mut result = vec![0u8; degree - 1];
            result.push(1);

            let mut root = 1u8;
            for _ in 0..degree {
                for j in 0..degree {
                    result[j] = QrCode::reed_solomon_multiply(result[j], root);
                    if j + 1 < result.len() {
                        result[j] ^= result[j + 1];
                    }
                }
                root = QrCode::reed_solomon_multiply(root, 0x02);
            }
            result
        }

        fn reed_solomon_compute_remainder(data: &[u8], divisor: &[u8]) -> Vec<u8> {
            let mut result = vec![0u8; divisor.len()];
            for b in data {
                let factor = b ^ result.remove(0);
                result.push(0);
                for (x, &y) in result.iter_mut().zip(divisor.iter()) {
                    *x ^= QrCode::reed_solomon_multiply(y, factor);
                }
            }
            result
        }

        fn reed_solomon_multiply(x: u8, y: u8) -> u8 {
            let mut z = 0u8;
            for i in (0..8).rev() {
                z = (z << 1) ^ ((z >> 7) * 0x1D);
                z ^= ((y >> i) & 1) * x;
            }
            z
        }
    }

    /*---- 罚分辅助 ----*/

    struct FinderPenalty {
        qr_size: i32,
        run_history: [i32; 7],
    }

    impl FinderPenalty {
        pub fn new(size: i32) -> Self {
            Self {
                qr_size: size,
                run_history: [0i32; 7],
            }
        }

        pub fn add_history(&mut self, mut currentrunlength: i32) {
            if self.run_history[0] == 0 {
                currentrunlength += self.qr_size;
            }
            let rh = &mut self.run_history;
            let len = rh.len();
            rh.copy_within(0..len - 1, 1);
            rh[0] = currentrunlength;
        }

        pub fn count_patterns(&self) -> i32 {
            let rh = &self.run_history;
            let n = rh[1];
            debug_assert!(n <= self.qr_size * 3);
            let core = n > 0 && rh[2] == n && rh[3] == n * 3 && rh[4] == n && rh[5] == n;
            i32::from(core && rh[0] >= n * 4 && rh[6] >= n)
                + i32::from(core && rh[6] >= n * 4 && rh[0] >= n)
        }

        pub fn terminate_and_count(
            mut self,
            currentruncolor: bool,
            mut currentrunlength: i32,
        ) -> i32 {
            if currentruncolor {
                self.add_history(currentrunlength);
                currentrunlength = 0;
            }
            currentrunlength += self.qr_size;
            self.add_history(currentrunlength);
            self.count_patterns()
        }
    }

    /*---- 常量与表 ----*/

    const PENALTY_N1: i32 = 3;
    const PENALTY_N2: i32 = 3;
    const PENALTY_N3: i32 = 40;
    const PENALTY_N4: i32 = 10;

    static ECC_CODEWORDS_PER_BLOCK: [[i8; 41]; 4] = [
        // 版本 0 为占位非法值。行顺序：Low、Medium、Quartile、High
        [
            -1, 7, 10, 15, 20, 26, 18, 20, 24, 30, 18, 20, 24, 26, 30, 22, 24, 28, 30, 28, 28, 28,
            28, 30, 30, 26, 28, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30,
        ],
        [
            -1, 10, 16, 26, 18, 24, 16, 18, 22, 22, 26, 30, 22, 22, 24, 24, 28, 28, 26, 26, 26, 26,
            28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28,
        ],
        [
            -1, 13, 22, 18, 26, 18, 24, 18, 22, 20, 24, 28, 26, 24, 20, 30, 24, 28, 28, 26, 30, 28,
            30, 30, 30, 30, 28, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30,
        ],
        [
            -1, 17, 28, 22, 16, 22, 28, 26, 26, 24, 28, 24, 28, 22, 24, 24, 30, 28, 28, 26, 28, 30,
            24, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30,
        ],
    ];

    static NUM_ERROR_CORRECTION_BLOCKS: [[i8; 41]; 4] = [
        // 版本 0 为占位非法值。行顺序：Low、Medium、Quartile、High
        [
            -1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 4, 4, 4, 4, 4, 6, 6, 6, 6, 7, 8, 8, 9, 9, 10, 12, 12,
            12, 13, 14, 15, 16, 17, 18, 19, 19, 20, 21, 22, 24, 25,
        ],
        [
            -1, 1, 1, 1, 2, 2, 4, 4, 4, 5, 5, 5, 8, 9, 9, 10, 10, 11, 13, 14, 16, 17, 17, 18, 20,
            21, 23, 25, 26, 28, 29, 31, 33, 35, 37, 38, 40, 43, 45, 47, 49,
        ],
        [
            -1, 1, 1, 2, 2, 4, 4, 6, 6, 8, 8, 8, 10, 12, 16, 12, 17, 16, 18, 21, 20, 23, 23, 25,
            27, 29, 34, 34, 35, 38, 40, 43, 45, 48, 51, 53, 56, 59, 62, 65, 68,
        ],
        [
            -1, 1, 1, 2, 4, 4, 4, 5, 6, 8, 8, 11, 11, 16, 16, 18, 16, 19, 21, 25, 25, 25, 34, 30,
            32, 35, 37, 40, 42, 45, 48, 51, 54, 57, 60, 63, 66, 70, 74, 77, 81,
        ],
    ];

    /*---- 纠错级别 ----*/

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum QrCodeEcc {
        Low,
        Medium,
        Quartile,
        High,
    }

    impl QrCodeEcc {
        fn ordinal(self) -> usize {
            match self {
                QrCodeEcc::Low => 0,
                QrCodeEcc::Medium => 1,
                QrCodeEcc::Quartile => 2,
                QrCodeEcc::High => 3,
            }
        }

        fn format_bits(self) -> u8 {
            match self {
                QrCodeEcc::Low => 1,
                QrCodeEcc::Medium => 0,
                QrCodeEcc::Quartile => 3,
                QrCodeEcc::High => 2,
            }
        }
    }

    /*---- 数据分段 ----*/

    struct QrSegment {
        mode: QrSegmentMode,
        numchars: usize,
        data: Vec<bool>,
    }

    impl QrSegment {
        fn make_bytes(data: &[u8]) -> Self {
            let mut bb = BitBuffer(Vec::with_capacity(data.len() * 8));
            for &b in data {
                bb.append_bits(u32::from(b), 8);
            }
            QrSegment::new(QrSegmentMode::Byte, data.len(), bb.0)
        }

        fn new(mode: QrSegmentMode, numchars: usize, data: Vec<bool>) -> Self {
            Self {
                mode,
                numchars,
                data,
            }
        }

        fn get_total_bits(segs: &[Self], version: Version) -> Option<usize> {
            let mut result = 0usize;
            for seg in segs {
                let ccbits = seg.mode.num_char_count_bits(version);
                if let Some(limit) = 1usize.checked_shl(ccbits.into())
                    && seg.numchars >= limit
                {
                    return None;
                }
                result = result.checked_add(4 + usize::from(ccbits))?;
                result = result.checked_add(seg.data.len())?;
            }
            Some(result)
        }
    }

    /*---- 分段模式 ----*/

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum QrSegmentMode {
        Byte,
    }

    impl QrSegmentMode {
        fn mode_bits(self) -> u32 {
            match self {
                QrSegmentMode::Byte => 0x4,
            }
        }

        fn num_char_count_bits(self, ver: Version) -> u8 {
            match self {
                QrSegmentMode::Byte => [8, 16, 16][usize::from((ver.value() + 7) / 17)],
            }
        }
    }

    /*---- 位缓冲 ----*/

    struct BitBuffer(Vec<bool>);

    impl BitBuffer {
        fn append_bits(&mut self, val: u32, len: u8) {
            assert!(len <= 31 && val >> len == 0, "Value out of range");
            self.0
                .extend((0..i32::from(len)).rev().map(|i| get_bit(val, i)));
        }
    }

    /*---- 其他值类型 ----*/

    #[derive(Debug, Clone)]
    pub enum DataTooLong {
        SegmentTooLong,
        DataOverCapacity(usize, usize),
    }

    impl std::error::Error for DataTooLong {}

    impl std::fmt::Display for DataTooLong {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            match *self {
                Self::SegmentTooLong => write!(f, "Segment too long"),
                Self::DataOverCapacity(datalen, maxcapacity) => write!(
                    f,
                    "Data length = {datalen} bits, Max capacity = {maxcapacity} bits"
                ),
            }
        }
    }

    #[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
    pub struct Version(u8);

    impl Version {
        pub const MIN: Version = Version(1);
        pub const MAX: Version = Version(40);

        pub const fn new(ver: u8) -> Self {
            assert!(ver >= 1 && ver <= 40, "Version number out of range");
            Self(ver)
        }

        pub const fn value(self) -> u8 {
            self.0
        }
    }

    #[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
    pub struct Mask(u8);

    impl Mask {
        pub const fn new(mask: u8) -> Self {
            assert!(mask <= 7, "Mask value out of range");
            Self(mask)
        }

        pub const fn value(self) -> u8 {
            self.0
        }
    }

    fn get_bit(x: u32, i: i32) -> bool {
        (x >> i) & 1 != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 贴近 B 站扫码登录的 URL（110 字节，QR version 6，41x41 模块）。
    const LOGIN_URL: &str = "https://passport.bilibili.com/h5-app/passport/login/scan?navhide=1&qrcode_key=0123456789abcdef0123456789abcdef";
    /// 66 字节，QR version 4，33x33 模块。
    const SHORT_URL: &str = "https://passport.bilibili.com/x/passport-login/web/qrcode/generate";

    fn width(line: &str) -> usize {
        line.chars().count()
    }

    fn module_size(line: &str) -> usize {
        width(line) - QUIET_ZONE_MODULES * 2
    }

    /// 读取已渲染输出中的模块颜色（不含安静区坐标）。
    fn module(lines: &[String], x: usize, y: usize) -> bool {
        let cell = lines[(y + QUIET_ZONE_MODULES) / 2]
            .chars()
            .nth(x + QUIET_ZONE_MODULES)
            .expect("module coordinate in range");
        if (y + QUIET_ZONE_MODULES).is_multiple_of(2) {
            matches!(cell, '▀' | '█')
        } else {
            matches!(cell, '▄' | '█')
        }
    }

    #[test]
    fn renders_known_login_url() {
        let lines = render_qr(LOGIN_URL).expect("known URL should render");
        assert!(!lines.is_empty());

        let w = width(&lines[0]);
        assert!(lines.iter().all(|line| width(line) == w));
        assert!(module_size(&lines[0]) >= 21);
        assert_eq!(module_size(&lines[0]) % 2, 1);

        // 输出只允许半块字符与空格，不能带 ANSI 转义。
        assert!(
            lines
                .iter()
                .flat_map(|line| line.chars())
                .all(|c| { matches!(c, '▀' | '▄' | '█' | ' ') })
        );
    }

    #[test]
    fn quiet_zone_surrounds_symbol() {
        let lines = render_qr(LOGIN_URL).expect("known URL should render");
        let w = width(&lines[0]);

        // 顶部/底部安静区各 2 个模块：首行与末行必须整行空白。
        assert!(lines[0].chars().all(|c| c == ' '));
        assert!(lines.last().unwrap().chars().all(|c| c == ' '));

        // 每一行左右都有安静区。
        for line in &lines {
            let chars: Vec<char> = line.chars().collect();
            assert!(chars[..QUIET_ZONE_MODULES].iter().all(|&c| c == ' '));
            assert!(chars[w - QUIET_ZONE_MODULES..].iter().all(|&c| c == ' '));
        }
    }

    #[test]
    fn renders_33_module_symbol() {
        let lines = render_qr(SHORT_URL).expect("short URL should render");
        assert_eq!(module_size(&lines[0]), 33);
        assert_eq!(lines.len(), (33 + QUIET_ZONE_MODULES * 2).div_ceil(2));
    }

    #[test]
    fn symbol_structure_is_valid() {
        let lines = render_qr(SHORT_URL).expect("short URL should render");
        let size = module_size(&lines[0]);
        assert_eq!(size, 33);

        // 三个 7x7 定位图形：外框深、内环浅、中心 3x3 深。
        for (ox, oy) in [(0, 0), (size - 7, 0), (0, size - 7)] {
            for dy in 0..7 {
                for dx in 0..7 {
                    let dist = (dx as i32 - 3).abs().max((dy as i32 - 3).abs());
                    assert_eq!(
                        module(&lines, ox + dx, oy + dy),
                        dist != 2,
                        "finder pattern mismatch at ({}, {})",
                        ox + dx,
                        oy + dy
                    );
                }
            }
        }

        // 定时图形从 8 到 size-9 交替，偶数坐标深。
        for i in 8..size - 8 {
            assert_eq!(module(&lines, i, 6), i % 2 == 0);
            assert_eq!(module(&lines, 6, i), i % 2 == 0);
        }

        // 固定深色模块。
        assert!(module(&lines, 8, size - 8));
    }

    #[test]
    fn fits_72x24_terminal() {
        let lines = render_qr(LOGIN_URL).expect("known URL should render");
        let w = width(&lines[0]);
        // 110 字节 → version 6（41x41 模块）→ 45 列 × 23 行。
        assert_eq!(module_size(&lines[0]), 41);
        assert_eq!(w, 45);
        assert_eq!(lines.len(), 23);
        assert!(w <= 72, "width {w} exceeds 72 columns");
        assert!(lines.len() <= 24, "height {} exceeds 24 rows", lines.len());
    }

    #[test]
    fn encodes_utf8_bytes() {
        let text = "https://example.test/登录?q=测试";
        let lines = render_qr(text).expect("UTF-8 text should render");
        assert!(!lines.is_empty());
        assert_eq!(lines, render_qr(text).unwrap());
    }

    #[test]
    fn empty_text_is_rejected() {
        assert!(render_qr("").is_err());
    }

    #[test]
    fn overlong_text_is_rejected() {
        // byte 模式在 Low 纠错下最多 2953 字节。
        let long = "a".repeat(4000);
        assert!(render_qr(&long).is_err());
    }
}
