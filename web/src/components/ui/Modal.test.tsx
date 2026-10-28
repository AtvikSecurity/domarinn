import { beforeAll, describe, expect, it } from "vitest";
import { useState } from "react";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Modal, ModalActions } from "./Modal";

// Radix uses pointer capture and measurement that jsdom does not implement.
beforeAll(() => {
  Element.prototype.hasPointerCapture = () => false;
  Element.prototype.setPointerCapture = () => {};
  Element.prototype.releasePointerCapture = () => {};
  Element.prototype.scrollIntoView = () => {};
});

function Harness({ withField = false }: { withField?: boolean }) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <button type="button" onClick={() => setOpen(true)}>
        Open
      </button>
      <Modal
        open={open}
        onOpenChange={setOpen}
        title="Delete thing"
        footer={
          <ModalActions
            onCancel={() => setOpen(false)}
            onConfirm={() => setOpen(false)}
          />
        }
      >
        {withField ? <input aria-label="Name" /> : null}
      </Modal>
    </>
  );
}

describe("Modal", () => {
  it("renders a labelled close button that dismisses the dialog", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await user.click(screen.getByRole("button", { name: "Open" }));
    const dialog = await screen.findByRole("dialog", { name: "Delete thing" });

    await user.click(screen.getByRole("button", { name: "Close" }));
    await waitFor(() => expect(dialog).not.toBeInTheDocument());
    // Focus returns to the trigger.
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Open" })).toHaveFocus(),
    );
  });

  it("keeps initial focus on the content and puts Close last in tab order", async () => {
    const user = userEvent.setup();
    render(<Harness withField />);
    await user.click(screen.getByRole("button", { name: "Open" }));
    await screen.findByRole("dialog");

    // The close button must not steal initial focus from the first field.
    expect(screen.getByRole("textbox", { name: "Name" })).toHaveFocus();
    await user.tab();
    expect(screen.getByRole("button", { name: "Cancel" })).toHaveFocus();
    await user.tab();
    expect(screen.getByRole("button", { name: "Confirm" })).toHaveFocus();
    await user.tab();
    expect(screen.getByRole("button", { name: "Close" })).toHaveFocus();
    // The focus trap wraps back into the dialog.
    await user.tab();
    expect(screen.getByRole("textbox", { name: "Name" })).toHaveFocus();
  });

  it("still closes on Escape", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await user.click(screen.getByRole("button", { name: "Open" }));
    const dialog = await screen.findByRole("dialog");
    await user.keyboard("{Escape}");
    await waitFor(() => expect(dialog).not.toBeInTheDocument());
  });
});
