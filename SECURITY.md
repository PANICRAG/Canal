# Security Policy

## Reporting a Vulnerability

If you discover a security vulnerability in Canal Engine, please report it responsibly.

**Do NOT open a public GitHub issue for security vulnerabilities.**

### How to Report

Use [GitHub Security Advisories](https://github.com/Aurumbach/canal-engine/security/advisories/new) to report vulnerabilities privately.

### What to Include

- Description of the vulnerability
- Steps to reproduce
- Impact assessment
- Suggested fix (if any)

### Response Timeline

- **Acknowledgment**: Within 72 hours
- **Assessment**: Within 1 week
- **Fix**: Depends on severity (critical: ASAP, high: 2 weeks, medium: next release)

## Supported Versions

| Version | Supported |
|---------|-----------|
| 0.1.x   | Yes       |

## Security Best Practices

When using Canal Engine:

- Never commit `.env` files with real credentials
- Set `ENVIRONMENT=production` in production to disable debug endpoints
- Use strong JWT secrets (`openssl rand -hex 32`)
- Configure tool permissions in `config/mcp-servers.yaml` (block dangerous tools, require confirmation)
- Set daily cost budgets in `config/llm-providers.yaml`
- Use Docker for code execution sandboxing
