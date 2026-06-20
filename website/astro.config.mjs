import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import tailwind from '@astrojs/tailwind';

export default defineConfig({
  site: 'https://alexknips.github.io',
  base: '/hivemind/',
  integrations: [
    starlight({
      title: 'HiveMind',
      description: 'Organizational decision memory for human + agent teams.',
      social: [
        { icon: 'github', label: 'GitHub', href: 'https://github.com/alexknips/hivemind' },
      ],
      sidebar: [
        {
          label: 'Getting Started',
          items: [
            { label: 'Quickstart', slug: 'getting-started/quickstart' },
            { label: 'Self-host', slug: 'getting-started/install' },
          ],
        },
        {
          label: 'Guides',
          items: [
            { label: 'MCP Setup', slug: 'guides/mcp-setup' },
            { label: 'Agent Capture', slug: 'guides/agent-capture' },
            { label: 'Human Review', slug: 'guides/human-review' },
          ],
        },
        {
          label: 'Concepts',
          items: [
            { label: 'Architecture', slug: 'concepts/architecture' },
            { label: 'Auth Model', slug: 'concepts/auth-model' },
            { label: 'Decision Graph', slug: 'concepts/decision-graph' },
          ],
        },
        {
          label: 'Reference',
          items: [
            { label: 'CLI Reference', slug: 'reference/cli' },
            { label: 'MCP Tools', slug: 'reference/mcp-tools' },
          ],
        },
      ],
      customCss: ['./src/styles/custom.css'],
    }),
    tailwind({ applyBaseStyles: false }),
  ],
});
