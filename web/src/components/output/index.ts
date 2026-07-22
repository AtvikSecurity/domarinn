// Public surface of the content-aware output viewer. Import from here rather
// than reaching into individual files; the heavy renderers (markdown, code)
// stay lazy behind OutputViewer.
export { OutputViewer } from "./OutputViewer";
export { RawText } from "./RawText";
export { JsonTree } from "./JsonTree";
export { detectContent, outputToString } from "./detect";
export type { ContentType, Detection } from "./detect";
