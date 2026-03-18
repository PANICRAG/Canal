//! Built-in Skills - Common skills for development workflows
//!
//! Provides pre-defined skills that can be overridden by user-defined skills.
//! These skills follow Claude Code patterns and PIV Loop framework.

use super::definition::Skill;

/// Enum representing builtin skill types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinSkill {
    Commit,
    Plan,
    BugFix,
    RalphLoop,
    CancelRalph,
    ParallelTasks,
    CreatePrd,
    EnvCheck,
    Deploy,
    DeployVerify,
}

impl BuiltinSkill {
    /// Get the skill name
    pub fn name(&self) -> &'static str {
        match self {
            BuiltinSkill::Commit => "commit",
            BuiltinSkill::Plan => "plan",
            BuiltinSkill::BugFix => "bug-fix",
            BuiltinSkill::RalphLoop => "ralph-loop",
            BuiltinSkill::CancelRalph => "cancel-ralph",
            BuiltinSkill::ParallelTasks => "parallel-tasks",
            BuiltinSkill::CreatePrd => "create_prd",
            BuiltinSkill::EnvCheck => "env-check",
            BuiltinSkill::Deploy => "deploy",
            BuiltinSkill::DeployVerify => "deploy-verify",
        }
    }

    /// Get all builtin skill types
    pub fn all() -> Vec<BuiltinSkill> {
        vec![
            BuiltinSkill::Commit,
            BuiltinSkill::Plan,
            BuiltinSkill::BugFix,
            BuiltinSkill::RalphLoop,
            BuiltinSkill::CancelRalph,
            BuiltinSkill::ParallelTasks,
            BuiltinSkill::CreatePrd,
            BuiltinSkill::EnvCheck,
            BuiltinSkill::Deploy,
            BuiltinSkill::DeployVerify,
        ]
    }

    /// Convert to a Skill instance
    pub fn to_skill(&self) -> Skill {
        match self {
            BuiltinSkill::Commit => create_commit_skill(),
            BuiltinSkill::Plan => create_plan_skill(),
            BuiltinSkill::BugFix => create_bug_fix_skill(),
            BuiltinSkill::RalphLoop => create_ralph_loop_skill(),
            BuiltinSkill::CancelRalph => create_cancel_ralph_skill(),
            BuiltinSkill::ParallelTasks => create_parallel_tasks_skill(),
            BuiltinSkill::CreatePrd => create_prd_skill(),
            BuiltinSkill::EnvCheck => create_env_check_skill(),
            BuiltinSkill::Deploy => create_deploy_skill(),
            BuiltinSkill::DeployVerify => create_deploy_verify_skill(),
        }
    }
}

/// Get all builtin skills
pub fn get_builtin_skills() -> Vec<Skill> {
    BuiltinSkill::all()
        .into_iter()
        .map(|s| s.to_skill())
        .collect()
}

/// Create the commit skill
fn create_commit_skill() -> Skill {
    Skill::builder("commit")
        .description("Create a git commit with proper message format")
        .prompt_template(COMMIT_PROMPT)
        .allowed_tools(vec!["Bash", "Read", "Glob", "Grep"])
        .argument_hint("[message]")
        .tag("git")
        .tag("vcs")
        .namespace("git")
        .priority(100)
        .builtin(true)
        .build()
}

/// Create the plan skill
fn create_plan_skill() -> Skill {
    Skill::builder("plan")
        .description("Create a development plan for implementing a feature or fixing a bug")
        .prompt_template(PLAN_PROMPT)
        .allowed_tools(vec!["Read", "Glob", "Grep"])
        .argument_hint("<task description>")
        .tag("planning")
        .tag("development")
        .priority(90)
        .builtin(true)
        .build()
}

/// Create the bug-fix skill
fn create_bug_fix_skill() -> Skill {
    Skill::builder("bug-fix")
        .description("Analyze and fix a bug in the codebase")
        .prompt_template(BUG_FIX_PROMPT)
        .allowed_tools(vec!["Read", "Write", "Edit", "Glob", "Grep", "Bash"])
        .argument_hint("<bug description>")
        .tag("debugging")
        .tag("development")
        .priority(80)
        .builtin(true)
        .build()
}

// ============================================================================
// Prompt Templates
// ============================================================================

const COMMIT_PROMPT: &str = r#"# Git Commit

Create a git commit following best practices.

## Task
$ARGUMENTS

## Instructions

1. First, run `git status` to see all untracked and modified files
2. Run `git diff` to review the staged and unstaged changes
3. Run `git log -5 --oneline` to see recent commit message style

Based on the changes:

4. Analyze all staged changes and draft a commit message that:
   - Summarizes the nature of the changes (new feature, bug fix, refactoring, etc.)
   - Uses the imperative mood (e.g., "Add feature" not "Added feature")
   - Is concise but descriptive (50 chars for title, wrap body at 72)
   - Follows the existing commit message style in the repository

5. Stage the appropriate files (prefer specific files over `git add -A`)
   - Do NOT commit files that likely contain secrets (.env, credentials, etc.)

6. Create the commit with the message ending with:
   ```
   Co-Authored-By: Claude <noreply@anthropic.com>
   ```

7. Verify the commit was successful with `git status`

## Important Notes
- NEVER push to remote unless explicitly requested
- NEVER amend existing commits unless explicitly requested
- NEVER skip pre-commit hooks
- If pre-commit hooks fail, fix the issues and create a NEW commit
"#;

const PLAN_PROMPT: &str = r#"# Development Plan

Create a detailed development plan for the following task.

## Task
$ARGUMENTS

## Instructions

1. **Understand the Task**
   - Read relevant files to understand the current codebase structure
   - Identify related components and their relationships
   - Note any existing patterns or conventions

2. **Analyze Requirements**
   - Break down the task into concrete requirements
   - Identify edge cases and potential issues
   - Consider backward compatibility if applicable

3. **Design the Solution**
   - Propose a high-level architecture
   - Identify files that need to be created or modified
   - List dependencies (libraries, services, etc.)

4. **Create Implementation Steps**
   - Break down into small, testable steps
   - Order steps by dependency (what needs to happen first)
   - Estimate complexity for each step

5. **Identify Risks and Mitigations**
   - What could go wrong?
   - How to detect problems early?
   - What's the rollback plan?

## Output Format

Provide the plan in the following structure:

```markdown
# Plan: [Task Title]

## Overview
[Brief description of what will be implemented]

## Requirements
- [ ] Requirement 1
- [ ] Requirement 2

## Implementation Steps

### Step 1: [Name]
**Files**: [list of files]
**Changes**: [what changes]
**Tests**: [what to test]

### Step 2: [Name]
...

## Risks
- Risk 1: [description] -> Mitigation: [solution]

## Success Criteria
- [ ] Criterion 1
- [ ] Criterion 2
```
"#;

const BUG_FIX_PROMPT: &str = r#"# Bug Fix

Analyze and fix the following bug.

## Bug Description
$ARGUMENTS

## Instructions

1. **Reproduce the Bug**
   - Understand the expected vs actual behavior
   - Identify steps to reproduce if not provided
   - Check for error messages or logs

2. **Locate the Root Cause**
   - Search for relevant code using Grep and Glob
   - Read related files to understand the flow
   - Trace the execution path
   - Form a hypothesis about the cause

3. **Analyze Impact**
   - What components are affected?
   - Are there related bugs that might share the same cause?
   - What's the blast radius of a fix?

4. **Implement the Fix**
   - Make minimal, targeted changes
   - Follow existing code patterns and style
   - Add comments explaining the fix if the logic is non-obvious

5. **Verify the Fix**
   - Run existing tests to ensure no regressions
   - Test the specific scenario that was failing
   - Consider edge cases

6. **Document**
   - Explain what the bug was
   - Explain why the fix works
   - Note any follow-up work needed

## Output Format

After fixing, provide a summary:

```markdown
## Bug Analysis

**Root Cause**: [explanation of why the bug occurred]

**Fix Applied**: [description of the changes made]

**Files Modified**:
- `path/to/file1.ts`: [brief description of change]
- `path/to/file2.ts`: [brief description of change]

**Testing**:
- [How to verify the fix]

**Follow-up** (if any):
- [Any additional work needed]
```
"#;

const RALPH_LOOP_PROMPT: &str = r#"# Ralph Wiggum Loop (PIV Loop)

Start an iterative self-referential AI development cycle using the PIV Loop framework.

## Task
$ARGUMENTS

## PIV Loop Framework

The PIV (Plan-Implement-Verify) Loop is an iterative development methodology:

### Phase 1: Plan (P)
- Analyze the current state of the task
- Break down into concrete, achievable steps
- Identify dependencies and blockers
- Set clear success criteria

### Phase 2: Implement (I)
- Execute the planned steps
- Make targeted, minimal changes
- Follow existing code patterns
- Document changes as you go

### Phase 3: Verify (V)
- Run tests to ensure correctness
- Check for regressions
- Verify against success criteria
- Identify any remaining issues

### Loop Iteration
After V phase:
- If all success criteria met → Complete
- If issues found → Return to P phase with learnings
- Maximum 5 iterations before escalation

## Instructions

1. **Initialize Loop State**
   - Create a loop state tracker
   - Record the initial task and context
   - Set iteration counter to 1

2. **Execute PIV Cycle**
   - Run through P → I → V phases
   - Document each phase's outcomes
   - Check success criteria after V

3. **Iterate or Complete**
   - If complete, summarize all changes
   - If not, analyze what went wrong and restart P

## Output Format

Track progress in this format:

```markdown
## PIV Loop Iteration [N]

### Plan Phase
- [ ] Step 1: [description]
- [ ] Step 2: [description]

### Implement Phase
- Changes made:
  - `file.ts`: [description]
- Commands run:
  - `npm test`: [result]

### Verify Phase
- Tests: [pass/fail]
- Criteria:
  - [x] Criterion 1
  - [ ] Criterion 2

### Decision: [CONTINUE/COMPLETE]
[Reason for decision]
```
"#;

const CANCEL_RALPH_PROMPT: &str = r#"# Cancel Ralph Wiggum Loop

Cancel an active Ralph Wiggum (PIV) loop execution.

## Instructions

1. Check for active PIV loop state
2. Record current progress
3. Clean up any temporary state
4. Summarize what was completed

## Output

Provide a summary of:
- Loop iterations completed
- Changes made during the loop
- Any incomplete work that needs attention
"#;

const PARALLEL_TASKS_PROMPT: &str = r#"# Parallel Task Execution

Execute multiple tasks in parallel waves based on dependency analysis.

## Tasks
$ARGUMENTS

## Instructions

1. **Analyze Dependencies**
   - Parse all tasks provided
   - Identify dependencies between tasks
   - Build a dependency graph

2. **Group into Waves**
   - Wave 1: Tasks with no dependencies
   - Wave 2: Tasks depending only on Wave 1
   - Wave N: Tasks depending on Wave N-1

3. **Execute Waves**
   - Execute all tasks in a wave concurrently
   - Wait for wave completion before starting next
   - Track results for each task

4. **Handle Failures**
   - If a task fails, mark dependent tasks as blocked
   - Continue with unblocked tasks
   - Report all failures at the end

## Output Format

```markdown
## Parallel Execution Plan

### Wave 1 (Parallelism: N)
- Task A: [status]
- Task B: [status]

### Wave 2 (Parallelism: M)
- Task C (depends on A): [status]
- Task D (depends on B): [status]

## Summary
- Total Tasks: X
- Completed: Y
- Failed: Z
- Blocked: W
```
"#;

const CREATE_PRD_PROMPT: &str = r#"# Create Product Requirements Document

Create a comprehensive PRD for a new feature or product.

## Feature/Product
$ARGUMENTS

## Instructions

1. **Research Phase**
   - Understand the problem being solved
   - Identify target users
   - Review existing solutions/competitors
   - Gather technical constraints

2. **Requirements Gathering**
   - Define functional requirements
   - Define non-functional requirements
   - Identify acceptance criteria
   - List out-of-scope items

3. **Technical Design**
   - High-level architecture
   - Data models
   - API specifications
   - Integration points

4. **Implementation Planning**
   - Break into milestones
   - Estimate complexity
   - Identify risks and mitigations

## Output Format

Create a PRD file at `.agent/prds/[feature-name].md`:

```markdown
# PRD: [Feature Name]

## Overview
[Brief description]

## Problem Statement
[What problem this solves]

## Target Users
[Who benefits]

## Requirements

### Functional Requirements
- FR-1: [requirement]
- FR-2: [requirement]

### Non-Functional Requirements
- NFR-1: [requirement]

### Out of Scope
- [what's not included]

## Technical Design

### Architecture
[diagram or description]

### Data Model
[schema]

### API
[endpoints]

## Milestones

### Phase 1: [Name]
- [ ] Task 1
- [ ] Task 2

## Risks
| Risk | Impact | Mitigation |
|------|--------|------------|
| ... | ... | ... |

## Success Metrics
- [metric 1]
- [metric 2]
```
"#;

const ENV_CHECK_PROMPT: &str = r#"# Environment Check

Verify the development environment is properly configured.

## Instructions

1. **Check Runtime Versions**
   - Node.js / Python / Rust version
   - Package manager versions
   - Required CLI tools

2. **Check Dependencies**
   - Verify package.json / Cargo.toml / requirements.txt
   - Check for missing dependencies
   - Verify version compatibility

3. **Check Configuration**
   - Environment variables
   - Config files (.env, etc.)
   - Credentials (without exposing values)

4. **Check Services**
   - Database connectivity
   - External API access
   - Required services running

## Output Format

```markdown
## Environment Check Results

### Runtime
- Node.js: [version] ✅
- npm: [version] ✅

### Dependencies
- Total packages: [N]
- Outdated: [M]
- Vulnerabilities: [X]

### Configuration
- .env: [present/missing]
- Required vars: [all present / missing: X, Y]

### Services
- Database: [connected/error]
- Redis: [connected/error]

## Issues Found
1. [issue description]

## Recommended Actions
1. [action to take]
```
"#;

const DEPLOY_PROMPT: &str = r#"# Deploy

Execute deployment to the specified environment.

## Environment
$ARGUMENTS

## Instructions

1. **Pre-deployment Checks**
   - Verify all tests pass
   - Check build succeeds
   - Verify environment configuration
   - Check for pending migrations

2. **Build Phase**
   - Clean build artifacts
   - Build production bundle
   - Generate assets

3. **Deploy Phase**
   - Push to deployment target
   - Run database migrations if needed
   - Update environment variables

4. **Post-deployment**
   - Verify deployment succeeded
   - Run smoke tests
   - Monitor for errors

## Output Format

```markdown
## Deployment Report

### Environment: [env]
### Timestamp: [datetime]

### Pre-checks
- [ ] Tests passing
- [ ] Build successful
- [ ] Config verified

### Deployment
- Build: [success/failed]
- Push: [success/failed]
- Migration: [success/skipped/failed]

### Verification
- Health check: [pass/fail]
- Smoke tests: [pass/fail]

### Notes
[Any issues or observations]
```
"#;

const DEPLOY_VERIFY_PROMPT: &str = r#"# Deploy Verification

Verify a deployment is working correctly.

## Deployment
$ARGUMENTS

## Instructions

1. **Health Checks**
   - Check application health endpoints
   - Verify service is responding
   - Check resource utilization

2. **Functional Tests**
   - Run smoke test suite
   - Verify critical user flows
   - Check integration points

3. **Performance Check**
   - Response time within SLA
   - No memory leaks
   - Database connections stable

4. **Log Analysis**
   - Check for errors
   - Warning patterns
   - Anomalies

## Output Format

```markdown
## Deployment Verification Report

### Service Status
- Health endpoint: [healthy/unhealthy]
- Response time: [Xms]
- Error rate: [X%]

### Functional Tests
- [x] User login
- [x] Core feature A
- [ ] Core feature B (failed: [reason])

### Performance
- P50 latency: [X]ms
- P99 latency: [X]ms
- CPU: [X%]
- Memory: [X%]

### Issues
1. [issue if any]

### Recommendation
[PROCEED / ROLLBACK / INVESTIGATE]
```
"#;

/// Create the ralph-loop skill
fn create_ralph_loop_skill() -> Skill {
    Skill::builder("ralph-loop")
        .description("Start a Ralph Wiggum loop - iterative self-referential AI development cycle")
        .prompt_template(RALPH_LOOP_PROMPT)
        .allowed_tools(vec!["Read", "Write", "Edit", "Glob", "Grep", "Bash"])
        .argument_hint("<task description>")
        .tag("development")
        .tag("iterative")
        .tag("piv-loop")
        .priority(95)
        .builtin(true)
        .build()
}

/// Create the cancel-ralph skill
fn create_cancel_ralph_skill() -> Skill {
    Skill::builder("cancel-ralph")
        .description("Cancel an active Ralph Wiggum loop")
        .prompt_template(CANCEL_RALPH_PROMPT)
        .allowed_tools(vec!["Read"])
        .argument_hint("(no arguments)")
        .tag("development")
        .tag("piv-loop")
        .priority(90)
        .builtin(true)
        .build()
}

/// Create the parallel-tasks skill
fn create_parallel_tasks_skill() -> Skill {
    Skill::builder("parallel-tasks")
        .description("Execute tasks in parallel waves based on dependency analysis")
        .prompt_template(PARALLEL_TASKS_PROMPT)
        .allowed_tools(vec!["Read", "Write", "Edit", "Glob", "Grep", "Bash"])
        .argument_hint("<task list>")
        .tag("development")
        .tag("parallel")
        .tag("orchestration")
        .priority(85)
        .builtin(true)
        .build()
}

/// Create the create_prd skill
fn create_prd_skill() -> Skill {
    Skill::builder("create_prd")
        .description("Create a Product Requirements Document")
        .prompt_template(CREATE_PRD_PROMPT)
        .allowed_tools(vec!["Read", "Write", "Glob", "Grep"])
        .argument_hint("<feature or product name>")
        .tag("planning")
        .tag("documentation")
        .namespace("prd")
        .priority(80)
        .builtin(true)
        .build()
}

/// Create the env-check skill
fn create_env_check_skill() -> Skill {
    Skill::builder("env-check")
        .description("Verify development environment configuration")
        .prompt_template(ENV_CHECK_PROMPT)
        .allowed_tools(vec!["Read", "Bash", "Glob"])
        .argument_hint("(no arguments)")
        .tag("devops")
        .tag("environment")
        .priority(70)
        .builtin(true)
        .build()
}

/// Create the deploy skill
fn create_deploy_skill() -> Skill {
    Skill::builder("deploy")
        .description("Execute deployment to specified environment")
        .prompt_template(DEPLOY_PROMPT)
        .allowed_tools(vec!["Read", "Bash", "Glob"])
        .argument_hint("<environment: staging|production>")
        .tag("devops")
        .tag("deployment")
        .priority(75)
        .builtin(true)
        .build()
}

/// Create the deploy-verify skill
fn create_deploy_verify_skill() -> Skill {
    Skill::builder("deploy-verify")
        .description("Verify a deployment is working correctly")
        .prompt_template(DEPLOY_VERIFY_PROMPT)
        .allowed_tools(vec!["Read", "Bash"])
        .argument_hint("<deployment identifier>")
        .tag("devops")
        .tag("deployment")
        .tag("verification")
        .priority(75)
        .builtin(true)
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_builtin_skills() {
        let skills = get_builtin_skills();
        assert_eq!(skills.len(), 10);

        let names: Vec<_> = skills.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"commit"));
        assert!(names.contains(&"plan"));
        assert!(names.contains(&"bug-fix"));
        assert!(names.contains(&"ralph-loop"));
        assert!(names.contains(&"cancel-ralph"));
        assert!(names.contains(&"parallel-tasks"));
        assert!(names.contains(&"create_prd"));
        assert!(names.contains(&"env-check"));
        assert!(names.contains(&"deploy"));
        assert!(names.contains(&"deploy-verify"));
    }

    #[test]
    fn test_builtin_skill_names() {
        assert_eq!(BuiltinSkill::Commit.name(), "commit");
        assert_eq!(BuiltinSkill::Plan.name(), "plan");
        assert_eq!(BuiltinSkill::BugFix.name(), "bug-fix");
        assert_eq!(BuiltinSkill::RalphLoop.name(), "ralph-loop");
        assert_eq!(BuiltinSkill::ParallelTasks.name(), "parallel-tasks");
    }

    #[test]
    fn test_builtin_skill_all() {
        let all = BuiltinSkill::all();
        assert_eq!(all.len(), 10);
    }

    #[test]
    fn test_commit_skill() {
        let skill = create_commit_skill();

        assert_eq!(skill.name, "commit");
        assert!(skill.description.contains("git commit"));
        assert!(skill.allowed_tools.contains(&"Bash".to_string()));
        assert!(skill.allowed_tools.contains(&"Read".to_string()));
        assert!(skill.is_builtin());
        assert_eq!(skill.metadata.namespace, Some("git".to_string()));
    }

    #[test]
    fn test_plan_skill() {
        let skill = create_plan_skill();

        assert_eq!(skill.name, "plan");
        assert!(skill.description.contains("development plan"));
        assert!(skill.allowed_tools.contains(&"Read".to_string()));
        assert!(skill.allowed_tools.contains(&"Glob".to_string()));
        assert!(skill.is_builtin());
    }

    #[test]
    fn test_bug_fix_skill() {
        let skill = create_bug_fix_skill();

        assert_eq!(skill.name, "bug-fix");
        assert!(skill.description.contains("bug"));
        assert!(skill.allowed_tools.contains(&"Read".to_string()));
        assert!(skill.allowed_tools.contains(&"Write".to_string()));
        assert!(skill.allowed_tools.contains(&"Edit".to_string()));
        assert!(skill.is_builtin());
    }

    #[test]
    fn test_commit_skill_prompt_has_arguments() {
        let skill = create_commit_skill();
        assert!(skill.prompt_template.contains("$ARGUMENTS"));
    }

    #[test]
    fn test_plan_skill_prompt_has_arguments() {
        let skill = create_plan_skill();
        assert!(skill.prompt_template.contains("$ARGUMENTS"));
    }

    #[test]
    fn test_bug_fix_skill_prompt_has_arguments() {
        let skill = create_bug_fix_skill();
        assert!(skill.prompt_template.contains("$ARGUMENTS"));
    }

    #[test]
    fn test_builtin_skill_to_skill() {
        let skill = BuiltinSkill::Commit.to_skill();
        assert_eq!(skill.name, "commit");

        let skill = BuiltinSkill::Plan.to_skill();
        assert_eq!(skill.name, "plan");

        let skill = BuiltinSkill::BugFix.to_skill();
        assert_eq!(skill.name, "bug-fix");
    }

    #[test]
    fn test_builtin_skills_have_tags() {
        let skills = get_builtin_skills();

        for skill in skills {
            assert!(
                !skill.metadata.tags.is_empty(),
                "Skill {} should have tags",
                skill.name
            );
        }
    }

    #[test]
    fn test_builtin_skills_have_priority() {
        let skills = get_builtin_skills();

        for skill in skills {
            assert!(
                skill.metadata.priority > 0,
                "Skill {} should have positive priority",
                skill.name
            );
        }
    }

    #[test]
    fn test_builtin_skills_have_argument_hints() {
        let skills = get_builtin_skills();

        for skill in skills {
            assert!(
                skill.argument_hint.is_some(),
                "Skill {} should have argument hint",
                skill.name
            );
        }
    }

    #[test]
    fn test_commit_skill_render_prompt() {
        let skill = create_commit_skill();
        let rendered = skill.render_prompt(Some("fix: resolve login issue"));

        assert!(rendered.contains("fix: resolve login issue"));
        assert!(rendered.contains("Git Commit"));
    }

    #[test]
    fn test_plan_skill_render_prompt() {
        let skill = create_plan_skill();
        let rendered = skill.render_prompt(Some("implement user authentication"));

        assert!(rendered.contains("implement user authentication"));
        assert!(rendered.contains("Development Plan"));
    }

    #[test]
    fn test_bug_fix_skill_render_prompt() {
        let skill = create_bug_fix_skill();
        let rendered = skill.render_prompt(Some("login button not responding"));

        assert!(rendered.contains("login button not responding"));
        assert!(rendered.contains("Bug Fix"));
    }

    #[test]
    fn test_commit_restricts_write() {
        let skill = create_commit_skill();
        // Commit skill should NOT have Write - it only reads and uses git
        assert!(!skill.allowed_tools.contains(&"Write".to_string()));
    }

    #[test]
    fn test_plan_is_read_only() {
        let skill = create_plan_skill();
        // Plan skill should be read-only
        assert!(!skill.allowed_tools.contains(&"Write".to_string()));
        assert!(!skill.allowed_tools.contains(&"Edit".to_string()));
        assert!(!skill.allowed_tools.contains(&"Bash".to_string()));
    }
}
