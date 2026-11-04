// Declarative tests only: page content and model arguments never become JavaScript.
export const actions = ['navigate', 'click', 'fill', 'press', 'select', 'check', 'expect_visible', 'expect_text', 'expect_url', 'expect_title', 'expect_count'];
const targetActions = new Set(['click', 'fill', 'press', 'select', 'check', 'expect_visible', 'expect_text', 'expect_count']);
const fields = {
  navigate: ['action', 'url'], click: ['action', 'target'], fill: ['action', 'target', 'value'],
  press: ['action', 'target', 'value'], select: ['action', 'target', 'value'], check: ['action', 'target', 'checked'],
  expect_visible: ['action', 'target'], expect_text: ['action', 'target', 'value'],
  expect_url: ['action', 'value'], expect_title: ['action', 'value'], expect_count: ['action', 'target', 'count'],
};
export function validateSteps(steps) {
  if (!Array.isArray(steps) || !steps.length || steps.length > 40) throw new Error('Use 1–40 test steps.');
  for (const s of steps) {
    if (!s || !actions.includes(s.action) || Object.keys(s).some(k => !fields[s.action].includes(k))) throw new Error('Invalid test step.');
    if (targetActions.has(s.action)) {
      const t = s.target;
      if (!t || !['role', 'label', 'text', 'testid', 'css'].includes(t.by) || typeof t.value !== 'string' || !t.value || t.value.length > 1024 ||
          Object.keys(t).some(k => !['by', 'value', 'name'].includes(k)) || (t.name !== undefined && (t.by !== 'role' || typeof t.name !== 'string' || t.name.length > 1024))) throw new Error('Invalid step target.');
    }
    const key = s.action === 'navigate' ? 'url' : ['fill', 'press', 'select', 'expect_text', 'expect_url', 'expect_title'].includes(s.action) ? 'value' : null;
    if (key && (typeof s[key] !== 'string' || s[key].length > 8192 || (key === 'url' && !s[key]))) throw new Error('Invalid step value.');
    if (s.action === 'check' && typeof s.checked !== 'boolean') throw new Error('Check requires checked: true or false.');
    if (s.action === 'expect_count' && (!Number.isInteger(s.count) || s.count < 0 || s.count > 1000)) throw new Error('Invalid expected count.');
  }
  if (steps[0].action !== 'navigate') throw new Error('The first step must navigate to a page.');
  return steps;
}
export function allowedUrl(value, baseUrl, origins) {
  const url = new URL(value, baseUrl);
  if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password || !origins.includes(url.origin)) throw new Error('This URL is outside the allowed websites.');
  return url.href;
}
export function locator(page, target) {
  switch (target.by) {
    case 'role': return page.getByRole(target.value, {name: target.name, exact: true});
    case 'label': return page.getByLabel(target.value, {exact: true});
    case 'text': return page.getByText(target.value, {exact: true});
    case 'testid': return page.getByTestId(target.value);
    case 'css': return page.locator(target.value);
    default: throw new Error('Unsupported target.');
  }
}
const q = JSON.stringify;
function targetCode(t) {
  return t.by === 'role' ? `page.getByRole(${q(t.value)}, { name: ${q(t.name)}, exact: true })` :
    t.by === 'css' ? `page.locator(${q(t.value)})` :
      `page.${{label: 'getByLabel', text: 'getByText', testid: 'getByTestId'}[t.by]}(${q(t.value)}${t.by === 'testid' ? '' : ', { exact: true }'})`;
}
export function generateSpec(title, steps, baseUrl, origins) {
  validateSteps(steps);
  const lines = steps.map(s => {
    const l = s.target ? targetCode(s.target) : '';
    switch (s.action) {
      case 'navigate': return `await page.goto(${q(allowedUrl(s.url, baseUrl, origins))});`;
      case 'click': return `await ${l}.click();`;
      case 'fill': return `await ${l}.fill(${q(s.value)});`;
      case 'press': return `await ${l}.press(${q(s.value)});`;
      case 'select': return `await ${l}.selectOption(${q(s.value)});`;
      case 'check': return `await ${l}.setChecked(${s.checked});`;
      case 'expect_visible': return `await expect(${l}).toBeVisible();`;
      case 'expect_text': return `await expect(${l}).toContainText(${q(s.value)});`;
      case 'expect_url': return `await expect(page).toHaveURL(${q(allowedUrl(s.value, baseUrl, origins))});`;
      case 'expect_title': return `await expect(page).toHaveTitle(${q(s.value)});`;
      case 'expect_count': return `await expect(${l}).toHaveCount(${s.count});`;
      default: throw new Error('Unsupported step.');
    }
  });
  return `import { test, expect } from '@playwright/test';\n\ntest(${q(title)}, async ({ page }) => {\n${lines.map(l => '  ' + l).join('\n')}\n});\n`;
}
