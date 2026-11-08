# Lab 3 进程创建与 Stride 调度

## 实现摘要

`spawn` 从目标 ELF 创建独立地址空间和内核栈，登记父子关系后把子进程加入就绪队列，并返回 PID。调度器选取累计 stride 最小的进程，按优先级增加其 stride。新进程默认优先级为 16，`set_priority` 接受至少为 2 的值。本章同时迁移了跨页写回的 `get_time` 和匿名页映射的 `mmap`、`munmap`。

## Stride 问答

### 8 位计数器回绕后谁先运行

按普通无符号数比较，仍会选中 P2。P2 运行后，真实的 stride 从 250 增至 260，存入 8 位计数器后变成 4。4 小于 P1 的 255，但真实的 260 大于 255，按调度规则应当轮到 P1。

### 最大与最小 stride 的差为何有上界

从一组 stride 均为零的进程出发，设每次调度前最小值为 `m`，所有进程都在区间 `[m, m + BigStride / 2]` 内。调度器只推进一个最小值进程。其优先级至少为 2，因此推进量至多为 `BigStride / 2`。其余进程的值不变，更新后所有值仍在原区间内，所以新一轮的最大值与最小值之差也至多为 `BigStride / 2`。

### 最大堆如何弹出真实 stride 最小的进程

取 `BigStride = u64::MAX` 时，差值上界小于 `2^63`。用 `wrapping_sub` 求模差，再解释为 `i64`，负值表示自身真实 stride 更小。Rust 的 `BinaryHeap` 先弹出比较结果较大的元素，因此反转顺序。题目假定 stride 两两不同，实际用于堆时还需实现与比较规则一致的 `Ord` 和相等关系。

```rust
fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
    let difference = self.0.wrapping_sub(other.0) as i64;
    Some(0i64.cmp(&difference))
}
```

以 8 位计数器为例，`125 - 255` 的模差为 126，故 125 的真实 stride 较大。`129 - 255` 的模差为 130，按 `i8` 解释为 -126，故 129 的真实 stride 较小。本章代码用 `u128` 累计 stride，队列扫描直接比较累计值。

## 来源与荣誉准则

本地练习的代码及报告由 Codex 生成。课程计分提交要求独立完成并如实声明。本练习材料仅供学习核对。[课程练习](https://learningos.cn/rCore-Tutorial-Guide/chapter5/4exercise.html) · [荣誉准则](https://learningos.cn/rCore-Tutorial-Guide/honorcode.html)
