import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { AboutPanel } from "./AboutPanel";

vi.mock("../lib/ipc", () => ({
  appVersion: vi.fn(() => Promise.resolve("0.1.0")),
  dataLocations: vi.fn(() =>
    Promise.resolve({
      database: "C:\\Users\\sam\\AppData\\Local\\Shelv\\shelv.db",
      webview_profile: "C:\\Users\\sam\\AppData\\Local\\Shelv\\EBWebView",
    }),
  ),
  revealDataFolder: vi.fn(() => Promise.resolve()),
  runsAtLogin: vi.fn(() => Promise.resolve(false)),
  setRunAtLogin: vi.fn(() => Promise.resolve()),
  ShelvError: class extends Error {},
}));

const noop = () => {
  // Intentionally empty.
};

describe("AboutPanel", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("names the running version", async () => {
    // Which build is this? is the other question a downloaded binary raises.
    render(<AboutPanel onClose={noop} />);
    expect(await screen.findByText(/Shelv 0\.1\.0/)).toBeInTheDocument();
  });

  it("shows both places Shelv keeps things", async () => {
    render(<AboutPanel onClose={noop} />);

    expect(
      await screen.findByText("C:\\Users\\sam\\AppData\\Local\\Shelv\\shelv.db"),
    ).toBeInTheDocument();
    expect(
      screen.getByText("C:\\Users\\sam\\AppData\\Local\\Shelv\\EBWebView"),
    ).toBeInTheDocument();
  });

  it("says the data belongs to the account rather than the program file", async () => {
    // The question that prompted this panel: a new build that already knows
    // your rules looks like a ghost until someone says it is deliberate.
    render(<AboutPanel onClose={noop} />);
    expect(
      await screen.findByText(/belong to your user account, not to the program file/i),
    ).toBeInTheDocument();
  });

  it("promises nothing about the backup drives themselves", async () => {
    render(<AboutPanel onClose={noop} />);
    expect(
      await screen.findByText(/Nothing here is on your backup drives/i),
    ).toBeInTheDocument();
  });

  it("opens the data folder without being told where it is", async () => {
    // The command takes no argument on purpose: a general "open this path"
    // capability would let this side ask the OS to launch anything.
    const user = userEvent.setup();
    const { revealDataFolder } = await import("../lib/ipc");
    render(<AboutPanel onClose={noop} />);

    await user.click(await screen.findByRole("button", { name: /Open data folder/ }));
    expect(revealDataFolder).toHaveBeenCalledWith();
  });

  it("reports a failure to open rather than appearing to have worked", async () => {
    const user = userEvent.setup();
    const { revealDataFolder } = await import("../lib/ipc");
    vi.mocked(revealDataFolder).mockRejectedValueOnce(new Error("no file manager"));

    render(<AboutPanel onClose={noop} />);
    await user.click(await screen.findByRole("button", { name: /Open data folder/ }));

    expect(await screen.findByText(/no file manager/)).toBeInTheDocument();
  });

  it("closes", async () => {
    const user = userEvent.setup();
    const closed = vi.fn();
    render(<AboutPanel onClose={closed} />);

    await user.click(await screen.findByRole("button", { name: "Close" }));
    expect(closed).toHaveBeenCalledOnce();
  });

  it("offers to start with the machine, and does not assume it", async () => {
    // A backup tool that adds itself to startup without asking has made a
    // decision that is not its to make.
    const user = userEvent.setup();
    render(<AboutPanel onClose={noop} />);

    const toggle = await screen.findByRole("checkbox", {
      name: /Start Shelv when I sign in/,
    });
    expect(toggle).not.toBeChecked();

    await user.click(toggle);
    const { setRunAtLogin } = await import("../lib/ipc");
    expect(setRunAtLogin).toHaveBeenCalledWith(true);
  });

  it("says what closing the window will do", async () => {
    // The surprise otherwise: closing it hides it, and a backup tool that
    // looks closed but is not has to say so somewhere.
    render(<AboutPanel onClose={noop} />);
    expect(
      await screen.findByText(/Closing the window hides it rather than quitting/),
    ).toBeInTheDocument();
  });
});
