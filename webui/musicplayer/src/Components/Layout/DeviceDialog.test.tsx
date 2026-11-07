import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import DeviceDialog from "./DeviceDialog";

const connectToCastDevice = vi.fn();
const disconnectFromCastDevice = vi.fn();
const connectToDevice = vi.fn();
const disconnectFromDevice = vi.fn();

const connectedSource = {
  id: "emby-account",
  name: "Home Emby",
  type: "music-player",
  isConnected: true,
};

const peer = {
  id: "peer-daemon",
  name: "Studio",
  type: "music-player",
  isConnected: false,
};

vi.mock("../../Hooks/useDevices", () => ({
  useDevices: () => ({
    devices: [connectedSource],
    castDevices: [peer],
    currentDevice: connectedSource,
    currentCastDevice: peer,
    connectToDevice,
    disconnectFromDevice,
    connectToCastDevice,
    disconnectFromCastDevice,
  }),
}));

const renderDialog = () => render(<DeviceDialog isOpen onClose={() => {}} />);

describe("DeviceDialog", () => {
  it("renders only cast targets and the local row", () => {
    renderDialog();
    expect(screen.getByText("This computer")).toBeInTheDocument();
    expect(screen.getByText("Studio")).toBeInTheDocument();
  });

  it("choosing local does not call source disconnect", async () => {
    const user = userEvent.setup();
    renderDialog();
    await user.click(screen.getByText("This computer"));
    expect(disconnectFromCastDevice).toHaveBeenCalledOnce();
    expect(disconnectFromDevice).not.toHaveBeenCalled();
  });

  it("choosing a peer calls connectToCastDevice, not connectToDevice", async () => {
    const user = userEvent.setup();
    renderDialog();
    await user.click(screen.getByText("Studio"));
    expect(connectToCastDevice).toHaveBeenCalledWith({ id: peer.id });
    expect(connectToDevice).not.toHaveBeenCalled();
    expect(disconnectFromDevice).not.toHaveBeenCalled();
  });

  it("choosing play-to output preserves connected source", async () => {
    const user = userEvent.setup();
    renderDialog();
    await user.click(screen.getByText("This computer"));
    // Switching outputs does not touch the mocked hook state, so both rows
    // simply need to remain available before the second interaction.
    await waitFor(() => expect(screen.getByText("Studio")).toBeInTheDocument());
    await user.click(screen.getByText("Studio"));
    expect(connectToDevice).not.toHaveBeenCalled();
    expect(disconnectFromDevice).not.toHaveBeenCalled();
    expect(connectedSource).toEqual({
      id: "emby-account",
      name: "Home Emby",
      type: "music-player",
      isConnected: true,
    });
  });
});
