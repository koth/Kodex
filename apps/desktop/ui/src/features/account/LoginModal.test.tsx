import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { LoginModal } from "./LoginModal";
import {
  remoteControlLogin,
  remoteControlLogout,
  remoteControlSendLoginCode,
} from "../../lib/tauri";

vi.mock("../../lib/tauri", async () => {
  const actual = await vi.importActual<typeof import("../../lib/tauri")>("../../lib/tauri");
  return {
    ...actual,
    remoteControlSendLoginCode: vi.fn(),
    remoteControlLogin: vi.fn(),
    remoteControlLogout: vi.fn(),
  };
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

function renderLoggedOut(overrides: { onClose?: () => void; onChanged?: () => void } = {}) {
  return render(
    <LoginModal
      loggedIn={false}
      accountEmail={null}
      onClose={overrides.onClose ?? vi.fn()}
      onChanged={overrides.onChanged ?? vi.fn()}
    />,
  );
}

describe("LoginModal", () => {
  it("sends a code then advances to the code step", async () => {
    vi.mocked(remoteControlSendLoginCode).mockResolvedValueOnce(undefined);
    renderLoggedOut();

    fireEvent.change(screen.getByLabelText("邮箱"), {
      target: { value: "user@example.com" },
    });
    fireEvent.click(screen.getByRole("button", { name: "发送验证码" }));

    await waitFor(() =>
      expect(remoteControlSendLoginCode).toHaveBeenCalledWith("user@example.com"),
    );
    // Code input is rendered only on the code step.
    expect(await screen.findByLabelText("验证码")).toBeInTheDocument();
  });

  it("logs in with email + code then notifies the caller", async () => {
    vi.mocked(remoteControlSendLoginCode).mockResolvedValueOnce(undefined);
    vi.mocked(remoteControlLogin).mockResolvedValueOnce(undefined);
    const onChanged = vi.fn();
    const onClose = vi.fn();
    render(
      <LoginModal loggedIn={false} accountEmail={null} onClose={onClose} onChanged={onChanged} />,
    );

    fireEvent.change(screen.getByLabelText("邮箱"), {
      target: { value: "user@example.com" },
    });
    fireEvent.click(screen.getByRole("button", { name: "发送验证码" }));
    const codeInput = await screen.findByLabelText("验证码");
    fireEvent.change(codeInput, { target: { value: "123456" } });
    fireEvent.click(screen.getByRole("button", { name: "登录" }));

    await waitFor(() =>
      expect(remoteControlLogin).toHaveBeenCalledWith("user@example.com", "123456"),
    );
    await waitFor(() => expect(onChanged).toHaveBeenCalledTimes(1));
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("surfaces the relay send-code error and stays on the email step", async () => {
    vi.mocked(remoteControlSendLoginCode).mockRejectedValueOnce("请求过于频繁，请稍后再试");
    renderLoggedOut();

    fireEvent.change(screen.getByLabelText("邮箱"), {
      target: { value: "user@example.com" },
    });
    fireEvent.click(screen.getByRole("button", { name: "发送验证码" }));

    await waitFor(() =>
      expect(screen.getByText("请求过于频繁，请稍后再试")).toBeInTheDocument(),
    );
    expect(screen.queryByLabelText("验证码")).not.toBeInTheDocument();
  });

  it("surfaces the relay login error on a wrong code", async () => {
    vi.mocked(remoteControlSendLoginCode).mockResolvedValueOnce(undefined);
    vi.mocked(remoteControlLogin).mockRejectedValueOnce("验证码错误");
    renderLoggedOut();

    fireEvent.change(screen.getByLabelText("邮箱"), {
      target: { value: "user@example.com" },
    });
    fireEvent.click(screen.getByRole("button", { name: "发送验证码" }));
    const codeInput = await screen.findByLabelText("验证码");
    fireEvent.change(codeInput, { target: { value: "000000" } });
    fireEvent.click(screen.getByRole("button", { name: "登录" }));

    await waitFor(() => expect(screen.getByText("验证码错误")).toBeInTheDocument());
  });

  it("shows the logged-in account and logs out", async () => {
    vi.mocked(remoteControlLogout).mockResolvedValueOnce(undefined);
    const onChanged = vi.fn();
    const onClose = vi.fn();
    render(
      <LoginModal
        loggedIn={true}
        accountEmail="user@example.com"
        onClose={onClose}
        onChanged={onChanged}
      />,
    );

    expect(screen.getByText("user@example.com")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "退出登录" }));

    await waitFor(() => expect(remoteControlLogout).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(onChanged).toHaveBeenCalledTimes(1));
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});
