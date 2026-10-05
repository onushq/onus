const TEMPLATES: Record<string, string> = {
  "order-placed": "<h1>Thanks for your order!</h1><p>Order {{orderId}} totals {{total}}.</p>",
  receipt: "<h1>Receipt</h1><p>{{body}}</p>",
};

function escapeHtml(value: string): string {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

export function renderTemplate(name: string, data: Record<string, string>): string {
  const template = TEMPLATES[name];
  if (template === undefined) {
    throw new Error(`Unknown template: ${name}`);
  }
  return template.replace(/\{\{(\w+)\}\}/g, (_match: string, key: string) => {
    const value = data[key];
    return value === undefined ? "" : escapeHtml(value);
  });
}
