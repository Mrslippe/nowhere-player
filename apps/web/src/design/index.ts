/*
 * design/ 统一入口。
 * 业务代码只从这里取设计资源（token + 基础组件），不直接引用 design 内部文件。
 */
import "./reset.css";
import "./tokens.css";
import "./themes.css";
import "./typography.css";

export * from "./components";
