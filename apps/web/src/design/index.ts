/*
 * design/ 统一入口。
 * 业务代码只从这里取设计资源（token + 基础组件），不直接引用 design 内部文件。
 */
import "./reset.css";
import "./tokens.css";
import "./themes.css";
import "./typography.css";
// 动效时长/缓动与 skin 无关；单独集中，组件只引用 --np-dur-* / --np-ease-*。
import "./motion.css";
// 结构层：先铺默认值（= plain），再由具体 skin 覆写；颜色始终留在 tokens/themes。
import "./skin/base.css";
import "./skin/plain.css";
import "./skin/phosphor.css";
// T-0056：8 个结构轴只按 <html data-structure-*> 切分支。
import "./structure.css";

import { installThemePersistence } from "../shared/theme";

export * from "./components";

// 两个窗口都会经过这个入口；主题持久化的挂载点暂放这里（见 shared/theme.ts 注释）。
installThemePersistence();
