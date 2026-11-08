/*
 * design/ 统一入口。
 * 业务代码只从这里取设计资源（token + 基础组件），不直接引用 design 内部文件。
 */
import "./reset.css";
import "./tokens.css";
import "./themes.css";
import "./typography.css";

import { installThemePersistence } from "../shared/theme";

export * from "./components";

// 两个窗口都会经过这个入口；主题持久化的挂载点暂放这里（见 shared/theme.ts 注释）。
installThemePersistence();
