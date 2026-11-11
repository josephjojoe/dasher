import { greet } from "vendor";
import "./side.js";
note('entry ' + greet());
document.getElementById('app').textContent = 'rendered';
export const rendered = document.getElementById('app').textContent;
import("./chunk.js").then((m) => note('chunk ' + m.default + ' ' + m.where));
