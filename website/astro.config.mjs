import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import tailwind from '@astrojs/tailwind';

export default defineConfig({
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
            { label: 'Install', slug: 'getting-started/install' },
            { label: 'Quickstart', slug: 'getting-started/quickstart' },
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
          label: 'Guides',
          items: [
            { label: 'Agent Capture', slug: 'guides/agent-capture' },
            { label: 'MCP Setup', slug: 'guides/mcp-setup' },
            { label: 'Human Review', slug: 'guides/human-review' },
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
