# preLATT-txt2segy 项目

## 项目概述
这是一个 Rust 语言写的地震/微地震监测小工具 ，用于将监测站/检波器采集的 TXT 文本数据转换为标准的 SEG-Y 地震数据格式。
可  批处理 / 实时处理  segy文件的转换。

## GUI 工具 

- 模式切换 ：批处理 vs 实时监视（互斥，切换时停止当前任务）
- 功能项 ：
  - 输入/输出目录选择（rfd 文件对话框）
  - 设备文件夹勾选（4 列布局、导入/导出列表）
  - 日期范围筛选
  - 命名模板预设（5 种）+ 自定义
  - 采样率（自动/手动）
  - SEG-Y 版本（Rev 0/1/2）
  - GPS 文件选择
  - 实时监视：等待窗口、输出时长、迟到数据策略、回填

## CLI 工具 
- 使用 clap derive 子命令：
  - convert -i <input> -o <output> [--gps file] [--sample-rate Hz] [--per-minute] [--revision rev0|rev1|rev2]
  - info -i <input> [--gps file] 显示数据集摘要
- 终端进度条：30 字符 ASCII bar（ = / - ），每 10% 更新一次，含 ETA
- 默认 sample rate = 5000 Hz；如果传入 0 或 5000.0 则启用自动检测
- use_gps_coords ：仅在传入 GPS 文件时为 true

有大佬有更好的同类软件，麻烦介绍给我，不慎感激。  Email：soul.shu@petalmail.com
