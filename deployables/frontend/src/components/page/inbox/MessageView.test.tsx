import { describe, expect, it } from "vitest";
import { render } from "@testing-library/react";
import { EmailBodyFrame } from "./MessageView";

describe("EmailBodyFrame", () => {
  it("keeps <body> attributes so mail can switch its own dark mode", () => {
    const { container } = render(
      <EmailBodyFrame html={'<html><body class="sc-darkmode" bgcolor="#F6F8FA"><p>x</p></body></html>'} />,
    );
    const srcdoc = container.querySelector("iframe")!.getAttribute("srcdoc")!;
    expect(srcdoc).toMatch(/<body[^>]*class="sc-darkmode"/);
  });
});
