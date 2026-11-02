// The document every page in this directory and below is rendered inside.
pub fn Layout(children: Element) -> Element {
    <html><main>{children}</main></html>
}
