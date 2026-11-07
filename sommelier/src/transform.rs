/*
Copyright 2026 Google LLC

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

     https://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
*/

//! 三套坐标系之间的纯逻辑变换，对应主线 C++ `sommelier-transform.cc` /
//! `sommelier-output.cc`。
//!
//! 1. Physical：host 物理像素（`OutputGeometry.physical_width/height`）。
//! 2. Virtual：guest (Xwayland / Wayland client) 看到的逻辑坐标，由 physical
//!    乘以 `ctx.scale` 得到（主线 `sl_transform_output_dimensions`）。
//! 3. Host Logical：host 通过 xdg_output / zaura_output 报告的 DIP 空间。
//!
//! `Normal` 模式用全局 `scale` 在 Host Logical <-> Virtual 间做乘/除；
//! `Direct` 模式（`--direct-scale`）用 `scale * device_scale_factor` 作为
//! Host Logical <-> Virtual 的每轴因子；`Stable` 模式（`--stable-scaling`）
//! 用 ceil/floor 代替 trunc 做取整，避免缩放抖动。
//!
//! 本模块刻意不持有 surface / window 状态，因此主线中依赖单个 surface 的
//! 分支（viewport_override、buffer contents_scale、scale_round_on_x/y、
//! per-surface own_scale）在这里由调用方负责；详见文件末尾的简化说明。

// 本模块刻意保留完整的变换 API 面（定点运算、按输出推断、damage/viewport
// 换算等），其中一部分当前只被单元测试或后续的 pointer/viewport 接线调用，
// 在非测试构建中会触发 dead_code lint。它们对应主线 sommelier 的公开变换
// 能力，属于有意保留而非遗漏。
#![allow(dead_code)]

/// `--scale` 的下界（主线 MIN_SCALE）。
pub const MIN_SCALE: f64 = 0.1;
/// `--scale` 的上界（主线 MAX_SCALE）。
pub const MAX_SCALE: f64 = 10.0;
/// `--dpi` 桶的下界（主线 MIN_DPI）。
pub const MIN_DPI: u32 = 72;
/// `--dpi` 桶的上界（主线 MAX_DPI）。
pub const MAX_DPI: u32 = 9600;
/// X11 应用约定的 "100%" DPI（主线 DEFACTO_DPI）。
pub const DEFACTO_DPI: f64 = 96.0;
/// 一英寸的毫米数（主线 INCH_IN_MM）。
pub const INCH_IN_MM: f64 = 25.4;
/// 上报给 guest 的 wl_output.scale 上限（主线 MAX_OUTPUT_SCALE）。
pub const MAX_OUTPUT_SCALE: i32 = 2;
/// XCURSOR_SIZE 的基准值（主线 XCURSOR_SIZE_BASE）。
pub const XCURSOR_SIZE_BASE: f64 = 24.0;
/// wl_fixed 的 24.8 小数点比例。
pub const WL_FIXED_ONE: i32 = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalingMode {
    /// 默认：全局 ctx.scale。
    Normal,
    /// `--direct-scale`：按输出 device_scale_factor 推导每轴因子。
    Direct,
    /// `--stable-scaling`：Normal 的取整策略变体（ceil/floor）。
    Stable,
}

/// 单个 host wl_output 的几何信息。
///
/// `width_mm` / `height_mm` / `subpixel` / `host_scale` 是 wl_output
/// geometry/scale 事件的透传数据，供后续接线读取；本纯逻辑模块主要消费
/// `x`/`y`/`physical_*`/`transform`/`device_scale_factor`。
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct OutputGeometry {
    pub x: i32,
    pub y: i32,
    pub physical_width: i32,
    pub physical_height: i32,
    pub width_mm: i32,
    pub height_mm: i32,
    pub subpixel: i32,
    pub transform: i32,
    pub host_scale: f64,
    pub device_scale_factor: f64,
}

#[derive(Debug, Clone)]
pub struct Transform {
    pub scale: f64,
    pub mode: ScalingMode,
    pub dpi_buckets: Vec<u32>,
    pub outputs: Vec<OutputGeometry>,
    pub stable_scale: bool,
}

// ---------------------------------------------------------------------------
// wl_fixed (24.8) 辅助函数
// ---------------------------------------------------------------------------

/// 纯整数 24.8 定点有理数乘法：`v * num / den`。
///
/// `v` 是 wl_fixed 原始值，不做浮点解码，因此没有 f32 精度损失。结果用
/// `i128` 中间量计算并 saturate 到 `i32`；`den == 0` 时原样返回（不 panic）。
/// 等价于主线 `wl_fixed_from_double(wl_fixed_to_double(v) * (num/den))`。
pub fn fixed_mul(v: i32, num: i32, den: i32) -> i32 {
    fixed_mul_frac(v, num as i64, den as i64)
}

/// wl_fixed 原始值与 f64 互转（全程 f64，不经过 f32，供接线层使用）。
/// 保留供 pointer/touch 定点坐标接线使用，当前变换走 `fixed_mul_frac`。
#[allow(dead_code)]
pub fn fixed_from_double(d: f64) -> i32 {
    sat_i32((d * WL_FIXED_ONE as f64).trunc())
}

/// wl_fixed 原始值与 f64 互转（全程 f64，不经过 f32，供接线层使用）。
#[allow(dead_code)]
pub fn fixed_to_double(v: i32) -> f64 {
    v as f64 / WL_FIXED_ONE as f64
}

/// [`fixed_mul`] 的高精度版本，允许 i64 的分子/分母。
///
/// `Transform` 的 fixed 变换把 f64 因子精确分解成二进制有理数（num/den 可达
/// 2^53），需要比 i32 更宽的参数。
pub fn fixed_mul_frac(v: i32, num: i64, den: i64) -> i32 {
    if den == 0 {
        return v;
    }
    let r = (v as i128) * (num as i128) / (den as i128);
    r.clamp(i32::MIN as i128, i32::MAX as i128) as i32
}

/// 把有限 f64 精确分解成 `num / den`（f64 本身是二进制有理数）。
///
/// 对 0.1 ~ 10 量级的因子，mantissa <= 2^53、den <= 2^56，都能落入 i64。
/// 极端指数或 NaN/Inf 返回 None，由调用方退化处理。
fn rational_from_f64(d: f64) -> Option<(i64, i64)> {
    if !d.is_finite() {
        return None;
    }
    let bits = d.to_bits();
    let negative = bits >> 63 != 0;
    let exp = ((bits >> 52) & 0x7ff) as i32;
    let frac = bits & ((1u64 << 52) - 1);

    let (mant, e): (i128, i32) = if exp == 0 {
        (frac as i128, -1074)
    } else {
        ((frac | (1u64 << 52)) as i128, exp - 1023 - 52)
    };

    let (mut num, mut den): (i128, i128) = if e >= 0 {
        if e > 60 {
            return None;
        }
        (mant << e, 1)
    } else {
        let shift = -e;
        if shift > 120 {
            return None;
        }
        (mant, 1i128 << shift)
    };

    // 约简掉 2 的公因子，尽量把 num/den 压进 i64。
    while den > 1 && num % 2 == 0 {
        num /= 2;
        den /= 2;
    }
    if num > i64::MAX as i128 || num < i64::MIN as i128 || den > i64::MAX as i128 {
        return None;
    }
    let n = num as i64;
    Some((if negative { -n } else { n }, den as i64))
}

/// wl_fixed 乘以一个 f64 因子（纯整数路径，无 f32 中间转换）。
fn fixed_scale(v: i32, factor: f64) -> i32 {
    match rational_from_f64(factor) {
        Some((num, den)) => fixed_mul_frac(v, num, den),
        None => {
            if !factor.is_finite() {
                return v;
            }
            let r = v as f64 * factor;
            if !r.is_finite() {
                return v;
            }
            sat_i32(r.trunc())
        }
    }
}

/// wl_fixed 除以一个 f64 因子（纯整数路径）。
fn fixed_div_scale(v: i32, factor: f64) -> i32 {
    match rational_from_f64(factor) {
        Some((num, den)) if num != 0 => fixed_mul_frac(v, den, num),
        _ => {
            if !factor.is_finite() || factor == 0.0 {
                return v;
            }
            let r = v as f64 / factor;
            if !r.is_finite() {
                return v;
            }
            sat_i32(r.trunc())
        }
    }
}

// ---------------------------------------------------------------------------
// 数值工具
// ---------------------------------------------------------------------------

fn sat_i32(v: f64) -> i32 {
    if !v.is_finite() {
        return 0;
    }
    v.clamp(i32::MIN as f64, i32::MAX as f64) as i32
}

fn sat_u32(v: f64) -> u32 {
    if !v.is_finite() {
        return 0;
    }
    v.clamp(0.0, u32::MAX as f64) as u32
}

fn sat_i64(v: f64) -> i64 {
    if !v.is_finite() {
        return 0;
    }
    v.clamp(i64::MIN as f64, i64::MAX as f64) as i64
}

/// 主线 `sl_output_apply_rotation`：normal/180/flipped/flipped_180 保持
/// 宽高，其余（90/270 及其 flipped）交换宽高。
fn apply_rotation(transform: i32, w: i32, h: i32) -> (i32, i32) {
    match transform {
        0 | 2 | 4 | 6 => (w, h),
        _ => (h, w),
    }
}

impl Transform {
    pub fn new(scale: f64, mode: ScalingMode, dpi_buckets: Vec<u32>) -> Self {
        // 主线在解析 --scale / sl_calculate_scale_for_xwayland 时 clamp。
        let scale = if scale.is_finite() {
            scale.clamp(MIN_SCALE, MAX_SCALE)
        } else {
            1.0
        };
        // 主线解析 --dpi 时把每个桶 clamp 到 [MIN_DPI, MAX_DPI]。
        let dpi_buckets = dpi_buckets
            .into_iter()
            .map(|d| d.clamp(MIN_DPI, MAX_DPI))
            .collect();
        Transform {
            scale,
            mode,
            dpi_buckets,
            outputs: Vec::new(),
            // Stable 模式默认启用 stable 取整；也可在任意模式下手动打开。
            stable_scale: mode == ScalingMode::Stable,
        }
    }

    /// 按 host 逻辑 x 坐标排序（主线 `sl_output_geometry` 维护有序列表，
    /// `sl_output_update_output_x` 依此顺序累加 virt_x）。
    pub fn set_outputs(&mut self, mut outputs: Vec<OutputGeometry>) {
        outputs.sort_by_key(|o| o.x);
        self.outputs = outputs;
    }

    fn is_stable(&self) -> bool {
        self.stable_scale || self.mode == ScalingMode::Stable
    }

    /// Host Logical <-> Virtual 的每轴缩放因子。
    ///
    /// Normal/Stable：直接用 `scale`（主线里 ctx.scale 已含 DSF）。
    /// Direct：`virtual = physical * scale`，`logical ≈ physical / DSF`，
    /// 故 `xdg_scale = virtual / logical = scale * DSF`（主线
    /// `sl_output_init_dimensions_direct` 的 `host->xdg_scale_*`）。
    fn axis_factors(&self) -> (f64, f64) {
        if self.mode == ScalingMode::Direct {
            let dsf = self
                .outputs
                .first()
                .map(|o| {
                    if o.device_scale_factor.is_finite() && o.device_scale_factor > 0.0 {
                        o.device_scale_factor
                    } else if o.host_scale.is_finite() && o.host_scale > 0.0 {
                        o.host_scale
                    } else {
                        1.0
                    }
                })
                .unwrap_or(1.0);
            (self.scale * dsf, self.scale * dsf)
        } else {
            (self.scale, self.scale)
        }
    }

    /// guest 应该看到的 wl_output 整数 scale（主线
    /// `sl_output_get_host_output_state` 的 Wayland 分支）：
    /// `min(ceil(applied_scale / ctx.scale), MAX_OUTPUT_SCALE)`。
    /// Direct 模式主线强制 scale = 1。
    pub fn guest_output_scale(&self, host_scale: f64) -> i32 {
        if self.mode == ScalingMode::Direct {
            return 1;
        }
        if !host_scale.is_finite() || host_scale <= 0.0 {
            return 1;
        }
        let s = sat_i32((host_scale / self.scale).ceil());
        s.clamp(1, MAX_OUTPUT_SCALE)
    }

    /// guest 侧虚拟输出尺寸：所有输出横向排布，总宽为各输出（旋转后）
    /// 虚拟宽度之和，总高取最大值。
    pub fn virtual_dimensions(&self) -> (i32, i32) {
        let mut width: i64 = 0;
        let mut height: i64 = 0;
        for o in &self.outputs {
            let (vw, vh) = self.host_to_guest_size(o.physical_width, o.physical_height);
            let (rw, rh) = apply_rotation(o.transform, vw, vh);
            width += rw as i64;
            height = height.max(rh as i64);
        }
        (
            width.clamp(i32::MIN as i64, i32::MAX as i64) as i32,
            height.clamp(i32::MIN as i64, i32::MAX as i64) as i32,
        )
    }

    /// 主线 `sl_infer_output_for_host_position`：返回包含查询点、或
    /// Manhattan 距离最近的输出下标。
    pub fn output_for_host_position(&self, x: i32, y: i32) -> Option<usize> {
        let mut best: Option<usize> = None;
        let mut best_distance = i64::MAX;
        for (i, o) in self.outputs.iter().enumerate() {
            let ox = o.x as i64;
            let oy = o.y as i64;
            let right = ox + o.physical_width as i64;
            let bottom = oy + o.physical_height as i64;
            let px = x as i64;
            let py = y as i64;

            let dx = if px < ox {
                ox - px
            } else if px < right {
                0
            } else {
                px - right
            };
            let dy = if py < oy {
                oy - py
            } else if py < bottom {
                0
            } else {
                py - bottom
            };
            let distance = dx.saturating_add(dy);
            if distance < best_distance {
                best = Some(i);
                best_distance = distance;
                if distance == 0 {
                    break;
                }
            }
        }
        best
    }

    /// 主线 `sl_infer_output_for_guest_position`：输出在虚拟空间横向排布，
    /// 因此忽略 Y；越界时返回最左/最右的输出。
    pub fn output_for_guest_position(&self, x: i32, y: i32) -> Option<usize> {
        let _ = y;
        if self.outputs.is_empty() {
            return None;
        }
        let mut virt_x: i64 = 0;
        for (i, o) in self.outputs.iter().enumerate() {
            let (vw, vh) = self.host_to_guest_size(o.physical_width, o.physical_height);
            let (rw, _) = apply_rotation(o.transform, vw, vh);
            let end = virt_x + rw as i64;
            if (x as i64) >= virt_x && (x as i64) < end {
                return Some(i);
            }
            virt_x = end;
        }
        // virt_x 从 0 开始，故 x < 0 视为落在第一个输出左侧。
        if x < 0 {
            Some(0)
        } else {
            Some(self.outputs.len() - 1)
        }
    }

    /// Host Logical -> Virtual 坐标（主线 `sl_transform_host_to_guest`）。
    /// 非 stable 用 trunc，stable 用 ceil。
    pub fn host_to_guest(&self, x: i32, y: i32) -> (i32, i32) {
        let (sx, sy) = self.axis_factors();
        if self.is_stable() {
            (
                sat_i32((x as f64 * sx).ceil()),
                sat_i32((y as f64 * sy).ceil()),
            )
        } else {
            (
                sat_i32((x as f64 * sx).trunc()),
                sat_i32((y as f64 * sy).trunc()),
            )
        }
    }

    /// Virtual -> Host Logical 坐标（主线 `sl_transform_guest_to_host`）。
    /// 非 stable 用 trunc，stable 用 floor。
    pub fn guest_to_host(&self, x: i32, y: i32) -> (i32, i32) {
        let (sx, sy) = self.axis_factors();
        if self.is_stable() {
            (
                sat_i32((x as f64 / sx).floor()),
                sat_i32((y as f64 / sy).floor()),
            )
        } else {
            (
                sat_i32((x as f64 / sx).trunc()),
                sat_i32((y as f64 / sy).trunc()),
            )
        }
    }

    /// 24.8 定点版的 `host_to_guest`，参数是 wl_fixed 原始 i32 值。
    pub fn host_to_guest_fixed(&self, x: i32, y: i32) -> (i32, i32) {
        let (sx, sy) = self.axis_factors();
        (fixed_scale(x, sx), fixed_scale(y, sy))
    }

    /// 24.8 定点版的 `guest_to_host`，参数是 wl_fixed 原始 i32 值。
    pub fn guest_to_host_fixed(&self, x: i32, y: i32) -> (i32, i32) {
        let (sx, sy) = self.axis_factors();
        (fixed_div_scale(x, sx), fixed_div_scale(y, sy))
    }

    /// Physical -> Virtual 尺寸（主线 `sl_transform_output_dimensions`）。
    /// 非 stable 用 trunc，stable 用 floor（主线
    /// `stable_scale_size_host_to_guest`）。
    pub fn host_to_guest_size(&self, w: i32, h: i32) -> (i32, i32) {
        let conv = |d: i32| -> i32 {
            let r = d as f64 * self.scale;
            if self.is_stable() {
                sat_i32(r.floor())
            } else {
                sat_i32(r.trunc())
            }
        };
        (conv(w), conv(h))
    }

    /// Virtual -> Host Logical 尺寸（主线 `sl_transform_viewport_scale`）。
    /// Normal/Stable 用 ceil 避免内容被裁；Direct 走坐标变换（stable 时
    /// floor，否则 trunc）。
    pub fn guest_to_host_size(&self, w: i32, h: i32) -> (i32, i32) {
        let (sx, sy) = self.axis_factors();
        let conv = |d: i32, s: f64| -> i32 {
            let r = d as f64 / s;
            if self.mode == ScalingMode::Direct {
                if self.is_stable() {
                    sat_i32(r.floor())
                } else {
                    sat_i32(r.trunc())
                }
            } else {
                sat_i32(r.ceil())
            }
        };
        (conv(w, sx), conv(h, sy))
    }

    /// 主线 `sl_transform_damage_coord`：输入 guest damage 矩形 (x,y,w,h)，
    /// 返回变换后的 host damage 矩形 (x, y, width, height)，可直接喂给
    /// `wl_surface_damage`（主线用 `x1, y1, x2-x1, y2-y1`）。
    ///
    /// 非 Direct 分支会向外扩 1px 以覆盖滤波（主线注释），并 clamp 到
    /// `[INT_MIN/10, INT_MAX/10]`；Direct 分支不做 outset。
    pub fn damage_coord(&self, x: i32, y: i32, w: i32, h: i32) -> (i32, i32, i32, i32) {
        let x1 = x as i64;
        let y1 = y as i64;
        let x2 = x as i64 + w as i64;
        let y2 = y as i64 + h as i64;

        let (nx1, ny1, nx2, ny2) = if self.mode == ScalingMode::Direct {
            let (sx, sy) = self.axis_factors();
            let conv = |v: i64, s: f64| -> i64 {
                let r = v as f64 / s;
                sat_i64(if self.is_stable() {
                    r.floor()
                } else {
                    r.trunc()
                })
            };
            (conv(x1, sx), conv(y1, sy), conv(x2, sx), conv(y2, sy))
        } else {
            const MIN_SIZE: i64 = i32::MIN as i64 / 10;
            const MAX_SIZE: i64 = i32::MAX as i64 / 10;
            let lo = |v: i64, s: f64| -> i64 {
                let t = (v - 1).max(MIN_SIZE);
                sat_i64((t as f64 / s).trunc())
            };
            let hi = |v: i64, s: f64| -> i64 {
                let t = (v + 1).min(MAX_SIZE);
                sat_i64((t as f64 / s).ceil())
            };
            (
                lo(x1, self.scale),
                lo(y1, self.scale),
                hi(x2, self.scale),
                hi(y2, self.scale),
            )
        };

        (
            nx1.clamp(i32::MIN as i64, i32::MAX as i64) as i32,
            ny1.clamp(i32::MIN as i64, i32::MAX as i64) as i32,
            (nx2 - nx1).clamp(i32::MIN as i64, i32::MAX as i64) as i32,
            (ny2 - ny1).clamp(i32::MIN as i64, i32::MAX as i64) as i32,
        )
    }

    /// 主线 `sl_transform_viewport_scale`：把 guest 的 viewport destination
    /// 尺寸换算成 host logical 尺寸。Direct 模式下极小窗口可能算出 0，
    /// 主线强制至少 1 像素。
    pub fn viewport_scale(&self, dest_w: i32, dest_h: i32) -> (i32, i32) {
        let (w, h) = self.guest_to_host_size(dest_w, dest_h);
        if self.mode == ScalingMode::Direct {
            (w.max(1), h.max(1))
        } else {
            (w, h)
        }
    }

    /// fractional-scale：按本 API 规格把 `wire_scale/120` 除以 `ctx.scale`
    /// 后 round，再乘 120（量化到 120 的整数倍）。
    ///
    /// 注意：主线 `sommelier-fractional-scale.cc:44` 实际是
    /// `round(wire_scale / ctx->scale)`，保留 1/120 精度。若需要严格主线
    /// 等价，把本函数体改为
    /// `sat_u32((host_wire_scale as f64 / self.scale).round())`。
    pub fn preferred_scale(&self, host_wire_scale: u32) -> u32 {
        if !self.scale.is_finite() || self.scale <= 0.0 {
            return host_wire_scale;
        }
        let scaled = ((host_wire_scale as f64 / 120.0) / self.scale).round() * 120.0;
        sat_u32(scaled)
    }

    /// DPI：按 `--dpi` 桶选最接近 target DPI 的一个；空桶表示使用精确
    /// target DPI（主线 `ctx->dpi.size == 0` 时不量化）。
    ///
    /// target DPI 采用主线 X11 分支 `DEFACTO_DPI * device_scale_factor`；
    /// 返回 `px * 25.4 / dpi`（主线 `dpi_to_physical_mm`，隐式截断）。
    pub fn physical_size_for_dpi(&self, width_px: i32, height_px: i32) -> (i32, i32) {
        let dsf = self
            .outputs
            .first()
            .map(|o| o.device_scale_factor)
            .filter(|d| d.is_finite() && *d > 0.0)
            .unwrap_or(1.0);
        let target = DEFACTO_DPI * dsf;

        let dpi = if let Some(&first) = self.dpi_buckets.first() {
            let mut best = first;
            let mut best_dist = (best as f64 - target).abs();
            for &b in self.dpi_buckets.iter().skip(1) {
                let dist = (b as f64 - target).abs();
                // 主线用严格 `<`，平局保留先出现的桶。
                if dist < best_dist {
                    best = b;
                    best_dist = dist;
                }
            }
            best as f64
        } else {
            target
        };

        let mm = |px: i32| -> i32 { sat_i32((px as f64 * (INCH_IN_MM / dpi)).trunc()) };
        (mm(width_px), mm(height_px))
    }

    /// 主线 `XCURSOR_SIZE = (int)(24 * ctx.scale + 0.5)`。
    pub fn cursor_size(&self) -> u32 {
        sat_u32((XCURSOR_SIZE_BASE * self.scale + 0.5).floor())
    }

    /// 为单个窗口计算专属缩放（主线 `sl_transform_try_window_scale`）。
    ///
    /// `contents_*` 是窗口在 guest 虚拟空间的像素尺寸（主线
    /// `width_in_pixels/height_in_pixels`）；`host_*` 是该窗口在 host
    /// logical 空间的尺寸（主线中由 `guest_to_host` 得到的
    /// `logical_width/logical_height`）。传入非正值时本模块自行推算。
    ///
    /// 返回窗口专属的 x 轴缩放因子（主线 `surface->xdg_scale_x`）；y 轴因子
    /// 可用 `contents_height / logical_height` 同样算出。非 Direct 模式主线
    /// 为 no-op，这里返回全局默认因子。
    pub fn try_window_scale(
        &self,
        contents_width: i32,
        contents_height: i32,
        host_width: i32,
        host_height: i32,
    ) -> f64 {
        let default = self.axis_factors().0;
        if self.mode != ScalingMode::Direct {
            return default;
        }

        let (gw, gh) = self.guest_to_host(contents_width, contents_height);
        let lw = if host_width > 0 { host_width } else { gw };
        let lh = if host_height > 0 { host_height } else { gh };
        // 主线：logical 宽或高为 0 时强制使用全局缩放因子。
        if lw <= 0 || lh <= 0 {
            return default;
        }

        // 主线：若默认因子下 pixel -> logical -> pixel 能往返一致，则不干预。
        let (rw, rh) = self.host_to_guest(lw, lh);
        if rw == contents_width && rh == contents_height {
            return default;
        }
        contents_width as f64 / lw as f64
    }
}

// ---------------------------------------------------------------------------
// 对主线语义做的简化（详见最终报告）
// ---------------------------------------------------------------------------
// * Direct 模式的 per-output xdg logical size 由 device_scale_factor 近似推导
//   （virtual/logical = scale * DSF），因为没有 xdg_output 事件输入。
// * host 位置匹配使用 physical_width/height 作为矩形尺寸（主线混用 logical
//   的 x/y 与 mode 的 physical 宽高，此处保持一致）。
// * damage 的 buffer contents_scale 固定为 1.0（API 未暴露该参数）。
// * viewport_override / scale_round_on_x/y / per-surface own_scale 等依赖
//   surface 状态的分支留给接线层；try_window_scale 只返回计算出的因子。
// * guest_output_scale 采用 Wayland 分支；主线 X11 分支恒返回 1。
// * preferred_scale 按 API 注释量化到 120 整数倍，与主线源码不同。

#[cfg(test)]
mod tests {
    use super::*;

    fn out(x: i32, y: i32, w: i32, h: i32) -> OutputGeometry {
        OutputGeometry {
            x,
            y,
            physical_width: w,
            physical_height: h,
            width_mm: 0,
            height_mm: 0,
            subpixel: 0,
            transform: 0,
            host_scale: 1.0,
            device_scale_factor: 1.0,
        }
    }

    /// wl_fixed_from_double 的测试辅助。
    fn fx(v: f64) -> i32 {
        (v * WL_FIXED_ONE as f64).round() as i32
    }

    #[test]
    fn new_clamps_scale_and_dpi_buckets() {
        assert_eq!(Transform::new(0.01, ScalingMode::Normal, vec![]).scale, 0.1);
        assert_eq!(
            Transform::new(50.0, ScalingMode::Normal, vec![]).scale,
            10.0
        );
        assert_eq!(
            Transform::new(f64::NAN, ScalingMode::Normal, vec![]).scale,
            1.0
        );
        assert_eq!(
            Transform::new(1.5, ScalingMode::Stable, vec![]).stable_scale,
            true
        );
        assert_eq!(
            Transform::new(1.0, ScalingMode::Normal, vec![10, 200, 99_999]).dpi_buckets,
            vec![72, 200, 9600]
        );
    }

    #[test]
    fn host_to_guest_normal_scale_1_2_1_5() {
        assert_eq!(
            Transform::new(1.0, ScalingMode::Normal, vec![]).host_to_guest(10, -20),
            (10, -20)
        );
        assert_eq!(
            Transform::new(2.0, ScalingMode::Normal, vec![]).host_to_guest(10, 20),
            (20, 40)
        );
        // 主线非 stable 用 trunc（向零取整）。
        assert_eq!(
            Transform::new(1.5, ScalingMode::Normal, vec![]).host_to_guest(3, 5),
            (4, 7)
        );
        assert_eq!(
            Transform::new(1.5, ScalingMode::Normal, vec![]).host_to_guest(-3, -5),
            (-4, -7)
        );
    }

    #[test]
    fn guest_to_host_normal() {
        assert_eq!(
            Transform::new(2.0, ScalingMode::Normal, vec![]).guest_to_host(20, 41),
            (10, 20)
        );
        assert_eq!(
            Transform::new(1.5, ScalingMode::Normal, vec![]).guest_to_host(4, 7),
            (2, 4)
        );
        assert_eq!(
            Transform::new(1.5, ScalingMode::Normal, vec![]).guest_to_host(-4, -7),
            (-2, -4)
        );
    }

    #[test]
    fn stable_scaling_uses_ceil_and_floor() {
        let t = Transform::new(1.5, ScalingMode::Stable, vec![]);
        assert_eq!(t.host_to_guest(3, 5), (5, 8));
        assert_eq!(t.host_to_guest(-3, -5), (-4, -7));
        assert_eq!(t.guest_to_host(5, 8), (3, 5));
        assert_eq!(t.guest_to_host(-5, -8), (-4, -6));
    }

    #[test]
    fn direct_mode_uses_device_scale_factor() {
        let mut t = Transform::new(1.5, ScalingMode::Direct, vec![]);
        let mut o = out(0, 0, 1920, 1080);
        o.device_scale_factor = 2.0;
        t.set_outputs(vec![o]);
        // Host Logical <-> Virtual 用 1.5 * 2 = 3.0，
        // 而 Physical -> Virtual 仍只用 1.5。
        assert_eq!(t.host_to_guest(10, 10), (30, 30));
        assert_eq!(t.guest_to_host(30, 30), (10, 10));
        assert_eq!(t.virtual_dimensions(), (2880, 1620));
        assert_eq!(t.guest_output_scale(2.0), 1);
        assert_eq!(t.host_to_guest_fixed(fx(2.0), fx(2.0)), (fx(6.0), fx(6.0)));
    }

    #[test]
    fn fixed_mul_is_integer_and_precise() {
        assert_eq!(fixed_mul(256 * 1000, 3, 2), fx(1500.0));
        assert_eq!(fixed_mul(fx(12345.75), 1, 1), fx(12345.75));
        assert_eq!(fixed_mul(-256_000, 3, 2), -384_000);
        assert_eq!(fixed_mul_frac(256, 1, 256), 1); // 1/256 像素
                                                    // den == 0 不 panic。
        assert_eq!(fixed_mul(1234, 7, 0), 1234);
        // 大数不溢出 i32 中间量。
        assert_eq!(fixed_mul(100_000_000, 1, 1), 100_000_000);
    }

    #[test]
    fn fixed_transform_has_no_f32_rounding_loss() {
        let t = Transform::new(1.5, ScalingMode::Normal, vec![]);
        assert_eq!(
            t.host_to_guest_fixed(fx(12345.75), fx(-0.5)),
            (fx(18518.625), fx(-0.75))
        );

        // 0.9 无法用 f32 精确表示；f32 路径会得到 2303999 而不是 2304000。
        let t = Transform::new(0.9, ScalingMode::Normal, vec![]);
        assert_eq!(
            t.host_to_guest_fixed(fx(10000.0), fx(10000.0)),
            (fx(9000.0), fx(9000.0))
        );

        let t = Transform::new(2.0, ScalingMode::Normal, vec![]);
        assert_eq!(
            t.guest_to_host_fixed(fx(15.0), fx(-7.0)),
            (fx(7.5), fx(-3.5))
        );
    }

    #[test]
    fn size_transforms_trunc_floor_and_ceil() {
        let t = Transform::new(1.5, ScalingMode::Normal, vec![]);
        assert_eq!(t.host_to_guest_size(3, 5), (4, 7));
        assert_eq!(t.host_to_guest_size(-3, -5), (-4, -7));
        // guest->host 用 ceil，避免内容被裁。
        assert_eq!(t.guest_to_host_size(4, 7), (3, 5));

        let stable = Transform::new(1.5, ScalingMode::Stable, vec![]);
        // stable 的 host->guest 尺寸用 floor（与 trunc 区分）。
        assert_eq!(stable.host_to_guest_size(-3, -5), (-5, -8));
        assert_eq!(stable.guest_to_host_size(4, 7), (3, 5));
    }

    #[test]
    fn dpi_bucket_selection_and_exact_dpi() {
        let mut t = Transform::new(1.0, ScalingMode::Normal, vec![96, 120, 144]);
        let mut o = out(0, 0, 1920, 1080);
        o.device_scale_factor = 1.0;
        t.set_outputs(vec![o]);
        // target = 96 -> 选 96。
        assert_eq!(t.physical_size_for_dpi(1920, 1080), (508, 285));

        let mut o2 = out(0, 0, 1920, 1080);
        o2.device_scale_factor = 1.3; // target = 124.8 -> 最接近 120
        t.set_outputs(vec![o2]);
        assert_eq!(t.physical_size_for_dpi(1920, 1080), (406, 228));

        // 空桶 = 精确 DPI（124.8），不做量化。
        let mut exact = Transform::new(1.0, ScalingMode::Normal, vec![]);
        let mut o3 = out(0, 0, 1920, 1080);
        o3.device_scale_factor = 1.3;
        exact.set_outputs(vec![o3]);
        assert_eq!(exact.physical_size_for_dpi(1920, 1080), (390, 219));
    }

    #[test]
    fn multiple_outputs_are_laid_out_horizontally() {
        let mut t = Transform::new(1.0, ScalingMode::Normal, vec![]);
        // 故意乱序，set_outputs 应按 x 排序。
        let mut rotated = out(1920, 0, 1280, 720);
        rotated.transform = 1; // 90 度：交换宽高 -> 720 x 1280
        let normal = out(0, 0, 1920, 1080);
        t.set_outputs(vec![rotated, normal]);
        assert_eq!(t.outputs[0].x, 0);
        assert_eq!(t.virtual_dimensions(), (1920 + 720, 1280));

        let half = Transform::new(0.5, ScalingMode::Normal, vec![]);
        let mut half_t = half.clone();
        half_t.set_outputs(vec![out(0, 0, 1920, 1080), out(1920, 0, 1280, 720)]);
        assert_eq!(half_t.virtual_dimensions(), (960 + 640, 540));
    }

    #[test]
    fn output_for_host_and_guest_position() {
        let mut t = Transform::new(1.0, ScalingMode::Normal, vec![]);
        t.set_outputs(vec![out(0, 0, 1920, 1080), out(1920, 0, 1280, 720)]);

        assert_eq!(t.output_for_host_position(100, 100), Some(0));
        assert_eq!(t.output_for_host_position(2000, 100), Some(1));
        assert_eq!(t.output_for_host_position(-100, 100), Some(0));
        assert_eq!(t.output_for_host_position(4000, 100), Some(1));

        assert_eq!(t.output_for_guest_position(100, 0), Some(0));
        assert_eq!(t.output_for_guest_position(1920 + 100, 0), Some(1));
        assert_eq!(t.output_for_guest_position(-5, 0), Some(0));
        assert_eq!(t.output_for_guest_position(10000, 999), Some(1));

        let empty = Transform::new(1.0, ScalingMode::Normal, vec![]);
        assert_eq!(empty.output_for_host_position(0, 0), None);
        assert_eq!(empty.output_for_guest_position(0, 0), None);
        assert_eq!(empty.virtual_dimensions(), (0, 0));
    }

    #[test]
    fn viewport_scale_ceil_and_min_one() {
        assert_eq!(
            Transform::new(2.0, ScalingMode::Normal, vec![]).viewport_scale(16, 16),
            (8, 8)
        );
        assert_eq!(
            Transform::new(0.9, ScalingMode::Normal, vec![]).viewport_scale(16, 16),
            (18, 18)
        );
        assert_eq!(
            Transform::new(1.5, ScalingMode::Stable, vec![]).viewport_scale(16, 16),
            (11, 11)
        );

        let mut direct = Transform::new(1.5, ScalingMode::Direct, vec![]);
        direct.set_outputs(vec![out(0, 0, 1920, 1080)]);
        assert_eq!(direct.viewport_scale(16, 16), (10, 10));
        // 极小窗口在 Direct 下会被 clamp 到 1 像素。
        assert_eq!(direct.viewport_scale(1, 1), (1, 1));
    }

    #[test]
    fn preferred_scale_quantizes_to_120() {
        let t = Transform::new(1.0, ScalingMode::Normal, vec![]);
        assert_eq!(t.preferred_scale(120), 120);
        assert_eq!(t.preferred_scale(180), 240); // round(1.5) = 2
        assert_eq!(t.preferred_scale(0), 0);

        assert_eq!(
            Transform::new(2.0, ScalingMode::Normal, vec![]).preferred_scale(240),
            120
        );
        assert_eq!(
            Transform::new(0.5, ScalingMode::Normal, vec![]).preferred_scale(120),
            240
        );
    }

    #[test]
    fn cursor_size_is_24_times_scale() {
        assert_eq!(
            Transform::new(1.0, ScalingMode::Normal, vec![]).cursor_size(),
            24
        );
        assert_eq!(
            Transform::new(2.0, ScalingMode::Normal, vec![]).cursor_size(),
            48
        );
        assert_eq!(
            Transform::new(1.5, ScalingMode::Normal, vec![]).cursor_size(),
            36
        );
        assert_eq!(
            Transform::new(0.5, ScalingMode::Normal, vec![]).cursor_size(),
            12
        );
        assert_eq!(
            Transform::new(0.1, ScalingMode::Normal, vec![]).cursor_size(),
            2
        );
    }

    #[test]
    fn guest_output_scale_is_clamped_to_2() {
        let t = Transform::new(1.0, ScalingMode::Normal, vec![]);
        assert_eq!(t.guest_output_scale(1.0), 1);
        assert_eq!(t.guest_output_scale(2.0), 2);
        assert_eq!(t.guest_output_scale(3.5), 2);
        assert_eq!(t.guest_output_scale(0.5), 1);
        assert_eq!(t.guest_output_scale(f64::NAN), 1);

        assert_eq!(
            Transform::new(2.0, ScalingMode::Normal, vec![]).guest_output_scale(4.0),
            2
        );
        assert_eq!(
            Transform::new(1.0, ScalingMode::Direct, vec![]).guest_output_scale(2.0),
            1
        );
    }

    #[test]
    fn damage_coord_outsets_by_one_pixel() {
        // 主线 TransformDamageCoord_UnscaledWithoutDirectScale: 15,15,17,17
        assert_eq!(
            Transform::new(1.0, ScalingMode::Normal, vec![]).damage_coord(16, 16, 0, 0),
            (15, 15, 2, 2)
        );
        // sx = 1.6 -> floor(15/1.6) = 9, ceil(17/1.6) = 11
        assert_eq!(
            Transform::new(1.6, ScalingMode::Normal, vec![]).damage_coord(16, 16, 0, 0),
            (9, 9, 2, 2)
        );
        // Direct 分支没有 1px outset。
        let mut direct = Transform::new(1.5, ScalingMode::Direct, vec![]);
        direct.set_outputs(vec![out(0, 0, 1920, 1080)]);
        assert_eq!(direct.damage_coord(16, 16, 4, 4), (10, 10, 3, 3));
    }

    #[test]
    fn try_window_scale_only_applies_in_direct_mode() {
        // 非 Direct 是 no-op，返回全局默认因子。
        assert_eq!(
            Transform::new(1.0, ScalingMode::Normal, vec![]).try_window_scale(1000, 800, 500, 400),
            1.0
        );

        let mut t = Transform::new(1.5, ScalingMode::Direct, vec![]);
        t.set_outputs(vec![out(0, 0, 1920, 1080)]);
        // 默认因子下 (60,60) -> (90,90)，与 contents 不一致 -> 专属因子。
        assert!((t.try_window_scale(100, 100, 60, 60) - 100.0 / 60.0).abs() < 1e-12);
        // 往返一致 -> 不干预，返回默认因子。
        assert_eq!(t.try_window_scale(90, 90, 60, 60), 1.5);
        // host 尺寸非正时内部推算 logical = (66, 66)。
        assert!((t.try_window_scale(100, 100, 0, 0) - 100.0 / 66.0).abs() < 1e-12);
        // logical 为 0 时强制使用全局因子。
        assert_eq!(t.try_window_scale(0, 0, 0, 0), 1.5);
    }
}
