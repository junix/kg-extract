# KG 抽取 e2e 语料：模型调研摘录

> 本文档由 `~/projects/docs/research/` 下四篇中文调研报告的**逐字摘录**拼接而成，
> 作为 `kg-extract` 端到端评测的多语种语料：叙述文本为中文，实体名为 ASCII。

## ASR 模型调研

> 摘自 `ASR-模型调研.md`

### 3.1 主流模型元数据

| 模型 | 参数量 | 架构 | 库 | 许可证 | 支持语种 | 下载 / 赞 | 最近更新 | 备注 |
|---|---|---|---|---|---|---|---|---|
| `openai/whisper-large-v3` | 1.55B | Transformer 编码-解码 | transformers | apache-2.0 | 99 | 5.4M / 5776 | 2024-08 | 英语 SOTA,多语种 zero-shot 强 |
| `openai/whisper-large-v3-turbo` | ~809M | 同上,decoder 层数 32→4 | transformers | mit | 99 | 8.6M / 3063 | 2024-10 | 6× 速度,几乎不掉点 |
| `openai/whisper-base` | 74M | 同上 | transformers | mit | 99 | 3.4M / 271 | 2024-02 | 边缘设备常用 |
| `distil-whisper/distil-large-v3` | 756M | Whisper 蒸馏 | transformers | mit | 1(英) | 0.94M / 376 | 2026-04 | 6× 加速,WER 与 large-v3 差距 ≤1% |
| `Systran/faster-whisper-large-v3` | 1.55B | CTranslate2 量化 | ctranslate2 | mit | 99 | 1.0M / 589 | 2023-11 | int8 量化,4× 加速,部署最广 |
| **`nvidia/parakeet-tdt-0.6b-v2`** | 0.6B | FastConformer-TDT | nemo | cc-by-4.0 | 1(英) | 0.33M / **1486** | 2026-04 | HF ASR Leaderboard 英语第一 |
| `nvidia/parakeet-tdt-0.6b-v3` | 0.6B | 同上 | transformers | cc-by-4.0 | 25(EU) | 0.08M / 902 | 2026-05 | 升级多语种+标点+时间戳 |
| `mlx-community/parakeet-tdt-0.6b-v3` | 0.6B | MLX 转换版 | mlx | cc-by-4.0 | 25(EU) | 1.28M / 43 | 2025-08 | Apple Silicon 优化 |
| `nvidia/canary-1b-v2` | 1B | FastConformer | nemo | cc-by-4.0 | 25(EU)+ASR+翻译 | 0.11M / 390 | 2025-12 | ASR + 翻译双任务 |
| `nvidia/canary-qwen-2.5b` | 2.5B | FastConformer + Qwen LLM | nemo | cc-by-4.0 | 英 | 0.05M / 432 | 2026-04 | LLM 后端,可热词/上下文 |
| `nvidia/nemotron-speech-streaming-en-0.6b` | 0.6B | FastConformer-RNNT 流式 | nemo | cc-by-4.0 | 英 | 0.008M / 561 | 2026-05 | cache-aware,流式 |
| `nvidia/parakeet-ctc-1.1b` | 1.1B | CTC | nemo | cc-by-4.0 | 英 | 0.84M / 49 | 2025-09 | 工业级英文转写 |
| **`Qwen/Qwen3-ASR-1.7B`** | 1.7B | Qwen3-Omni 派生的多模态 | - | apache-2.0 | 30 + 22 中文方言 | 1.88M / **857** | 2026-01 | 含唱歌/含 BGM |
| `Qwen/Qwen3-ASR-0.6B` | 0.6B | 同上,小尺寸 | - | apache-2.0 | 30 + 22 中文方言 | 0.91M / 298 | 2026-01 | 吞吐 2000×(@concurrency=128) |
| `Qwen/Qwen3-ForcedAligner-0.6B` | 0.6B | NAR | - | apache-2.0 | 11 | 0.45M / 137 | 2026-01 | 时间戳对齐 |
| **`mistralai/Voxtral-Mini-4B-Realtime-2602`** | 4B | 自回归 + 因果音频编码 | vllm | apache-2.0 | 13 | 1.17M / 868 | 2026-03 | <500ms 流式,端侧 |
| `microsoft/Phi-4-multimodal-instruct` | 5.6B | 多模态 LLM | transformers | mit | 多 | 0.53M / 1600 | 2025-12 | 语音+翻译+摘要 |

## TTS 与 VAD 模型调研

> 摘自 `TTS-VAD-模型调研.md`

## 3. TTS 核心模型深度对比

### 3.1 元数据对比

| 模型 | 参数量 | 架构 / backbone | 库 | 许可证 | 语种 | 关键能力 | 下载 / 赞 | 更新 |
|---|---|---|---|---|---|---|---|---|
| **`hexgrad/Kokoro-82M`** | 82M | StyleTTS2 派生 | - | apache-2.0 | 英 | 超轻量,API <$1/M 字符 | 13.7M / 6267 | 2025-04 |
| **`coqui/XTTS-v2`** | ~880M | VITS + GPT-style | coqui | **cpml**(自定义) | 17 | zero-shot 克隆,多语种 | 10.0M / 3580 | 2023-12 |
| **`ResembleAI/chatterbox`** | 0.5B | Llama backbone + codec | chatterbox | **mit** | 23(含阿/俄/中) | 情绪夸张控制,水印 | 1.85M / 1606 | 2026-04 |
| **`Qwen3-TTS-12Hz-1.7B-CustomVoice`** | 1.7B | discrete multi-codebook LM | qwen-tts | apache-2.0 | 10 | 角色预设 + 多语种 | 1.94M / 1568 | 2026-01 |
| `Qwen3-TTS-12Hz-0.6B-CustomVoice` | 0.6B | 同上 | qwen-tts | apache-2.0 | 10 | 轻量版 | 0.95M / 152 | 2026-01 |
| `Qwen3-TTS-12Hz-1.7B-VoiceDesign` | 1.7B | 同上 | qwen-tts | apache-2.0 | 10 | **文字描述**生成音色 | 0.75M / 355 | 2026-01 |
| `Qwen3-TTS-12Hz-0.6B-Base` | 0.6B | 同上 | - | apache-2.0 | 10 | 3 秒克隆 | 0.74M / 242 | 2026-01 |
| **`SWivid/F5-TTS`** | 0.3B | Flow Matching | f5-tts | **cc-by-nc-4.0** | 中/英 | 零样本克隆,Emilia 数据集 | 0.66M / 1177 | 2025-03 |
| `suno/bark` | ~1B | GPT-style | transformers | mit | 多 | 含音乐/笑声,zero-shot | 0.02M / 1531 | 2023-10 |
| `FunAudioLLM/Fun-CosyVoice3-0.5B-2512` | 0.5B | LLM-based + flow | cosyvoice | apache-2.0 | 9 + 18 中文方言 | 多语种 + 跨语种克隆 | 0.16M / 561 | 2025-12 |
| **`openbmb/VoxCPM2`** | 2B | tokenizer-free diffusion AR | voxcpm | apache-2.0 | 30 | 48kHz 高保真,文字描述声纹 | 0.25M / 1371 | 2026-05 |
| `sesame/csm-1b` | 1B | decoder-only + audio | transformers | apache-2.0 | 英 | 拟人对话 | 0.25M / 2386(受限) | 2025 |
| `microsoft/VibeVoice-Realtime-0.5B` | 0.5B | Qwen2.5-0.5B + 7.5Hz tokenizer | transformers | **mit** | 英 + 9 | 流式 ~300ms,长音频 | 0.81M / 1231 | 2026-04 |
| `microsoft/VibeVoice-1.5B` | 1.5B | Qwen2.5-1.5B + σ-VAE | transformers | **mit** | 英/中 | **90 分钟 / 4 说话人** | 0.05M / 2386 | 2026-04 |

## Projects 模型测试矩阵

> 摘自 `Projects-模型测试矩阵.md`

## 6. Swift 仓库详情

### 6.1 `swift/vad-swift`(无独立 Tests)
- 模块:`AsrStream`, `Diarize`, `Download`, `EventPublisher`, `Turn`, `VadCLI`, `VaddSocket` 等
- **使用 `mlx-audio-swift` 的 `MLXAudioVAD` 库**
- **缺失**:无 `Tests/` 目录

### 6.2 `swift/qwen3-asr`(无独立 Tests)
- 通过 `mlx-audio-swift` 的 `MLXAudioSTT` 调 Qwen3-ASR
- 依赖:`MLXAudioSTT`, `MLXAudioCore`, `HuggingFace`, `ArgumentParser`
- **缺失**:无 `Tests/` 目录

### 6.3 `swift/vadd` — **Tests/VADKitTests/(4 套测试)**
| 测试文件 | 覆盖 |
|---|---|
| `EnergyVadTests.swift` | 能量 VAD 算法 |
| `SegmentBufferTests.swift` | 段缓冲 |
| `VadSegmenterTests.swift` | 段分割器 |
| `WavEncoderTests.swift` | WAV 编码 |
| 内嵌资源:`Sources/vadd/Resources/silero_vad.onnx`(Silero VAD v5 真实模型) | |

## AI 编程 Harness

> 摘自 `YC 内部 AI 架构（Garry Tan / Steve Yegge）`

**“使用 AI 编程代理的人比今天使用 Cursor 和聊天的工程师生产效率高 10 倍到 100 倍，并且比 2005 年时的谷歌员工高约 1000 倍。”**

这么夸张的数字，并不是Garry Tan自己随便说的，而是来自Steve Yegge——一位在美国程序员圈里的网红人物。Steve Yegge 曾在亚马逊工作7年、在谷歌任职13年，目前担任Sourcegraph的工程主管，他的职业生涯经历了从 1992 年开始，到2005年谷歌黄金时代，再到今天的AI时代，三十多年的技术演变。

Garry Tan——现任 Y Combinator 总裁兼首席执行官，在帖子里引用Steve的话时特别强调：这个数字是真的，他自己亲眼见过，也亲身实践过。

但最关键的一点是——实现10倍、100倍甚至1000倍生产力的人，和只提升2倍的人，用的其实是同一个AI模型。

Garry Tan 认为：**秘密不在于模型，而在于包裹模型的那个东西。**

我们一起来看看，被称为这个“东西”到底是什么！

**Harness 是产品**

在2026 年 3 月 31 日，Anthropic 意外地将 Claude Code 的51.2万行源代码上传到了 npm 注册中心。证实了Garry Tan 一直在 YC 所教授的一切：秘密不在于模型，而在于包裹模型的那个东西。

实时仓库上下文、提示缓存、专门构建的工具、上下文冗余最小化、结构化会话记忆、并行子代理——这些都不让模型变得更聪明，而是全部为模型提供恰当的上下文，在恰当的时间，不让它被噪音淹没。

Garry Tan把那个包裹器被称为“harness”。
