# scripts/tools

给**人**和**智能体**共用的外部工具。它们不属于产品代码，不进构建，只用于验收与调试。

> 存在理由：过去同一件事被重复造了四次（T-0008 实验室、T-0027 视觉回归、T-0028 phosphor、T-0031 原型）。
> 工具应该住在仓库里，而不是住在某个代理的上下文里。

## 清单

| 工具 | 用途 | 依赖 |
|---|---|---|
| `shot.mjs` | headless Edge 截一张 PNG | 系统 Edge |
| `png-hash.mjs` | 两组截图逐字节比较（视觉回归） | 无 |
| `pixel-diff.py` | 像素级差异（跨机器时用） | Pillow |
| `make-wav.mjs` | 生成测试音频（可带内嵌封面） | 无 |

## 常用命令

```bash
# 截图（固定 1×，跨次可比）
node scripts/tools/shot.mjs --url "file:///E:/…/preview/index.html" --out tmp/shot.png --w 1440 --h 900

# 视觉回归：同一份基线 vs 改动后
node scripts/tools/png-hash.mjs --dir tmp/before --dir tmp/after
#   → "N 相同 / M 不同"，退出码 0 表示零回归

# 端到端用的测试曲库（前 6 个带封面）
node scripts/tools/make-wav.mjs --out tmp/test-audio --count 10 --covers 6
```

## 为什么优先用哈希而不是像素 diff

我们的验收标准是**零视觉回归**。同一台机器、同一个 Edge、同样的 `deviceScaleFactor` 下，未改动的页面渲染结果是**逐字节相同**的（T-0027 / T-0028 实测：9 张图 SHA-256 全等）。

逐字节相等比"差异像素 < N"更硬，也不需要图像库。只有跨机器（字体抗锯齿不同）时才需要 `pixel-diff.py`。

## 约定

- 产物一律写到 `tmp/`（已 gitignore），**不要**提交截图。
- 需要长期留存的验截图，复制到 `docs/30-计划/截图/` 并在文档里引用。
- 工具改了要更新本文件；新增工具同理。
