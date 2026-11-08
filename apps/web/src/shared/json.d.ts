/**
 * Vitest / Vite 原生支持 JSON 导入，但本项目未开启 `resolveJsonModule`。
 * 这里只声明形状为 unknown，交给 `terminal-model.guard.ts` 做运行时校验。
 */
declare module "*.json" {
  const value: unknown;
  export default value;
}
