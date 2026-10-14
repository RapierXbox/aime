import { describe, expect, it } from "vitest";
import { supportsDarkMode } from "./emailColorScheme";

describe("supportsDarkMode", () => {
  it.each([
    ['<meta name="color-scheme" content="light dark"><p style="color:#000">x</p>'],
    ['<meta name="supported-color-schemes" content="light dark">'],
    ["<style>@media (prefers-color-scheme: dark){body{color:#fff}} body{color:#000}</style>"],
    ["<style>:root{color-scheme: light dark}</style>"],
    ['<div dir="ltr">Hi,<br>plain mail</div>'],
    ['<p style="border-color:red">only a border color</p>'],
  ])("dark-safe: %s", (html) => expect(supportsDarkMode(html)).toBe(true));

  it.each([
    ['<body style="background:#fff"><p>x</p></body>'],
    ['<table bgcolor="#ffffff"><tr><td>x</td></tr></table>'],
    ['<p style="color: #333">x</p>'],
    ["<style>.a{background-color:white}</style>"],
    ['<font color="red">x</font>'],
    ['<meta name="color-scheme" content="light"><p style="color:#000">x</p>'],
  ])("light-only: %s", (html) => expect(supportsDarkMode(html)).toBe(false));
});
