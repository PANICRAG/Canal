// DEPRECATED: Browser module replaced by canal-cv (CV8).
// These tests reference the old browser module API (with_browser_router).
// Will be rewritten against canal-cv API in a future session.
#![cfg(feature = "browser-legacy-tests")]

//! Browser Agent Integration Tests - Enterprise Management Scenarios
//!
//! These tests simulate real-world enterprise workflows:
//! - HR/Employee Management
//! - Project Management
//! - Finance/Accounting
//! - Customer Relations
//! - Internal Communications
//! - Recruitment
//!
//! Prerequisites:
//! - Chrome browser running with Canal extension installed and connected
//! - QWEN_API_KEY environment variable set
//! - Logged into Google account (for Gmail/Calendar/Docs tests)
//!
//! Run with: `cargo test --package gateway-core --test browser_agent_integration -- --ignored --nocapture`

use futures::StreamExt;
use gateway_core::agent::types::PermissionMode;
use gateway_core::agent::types::StreamEventSubtype;
use gateway_core::agent::{AgentFactory, AgentLoop, AgentMessage, ContentBlock};
use gateway_core::browser::{
    ExtensionManager, ExtensionManagerConfig, RouterMode as BrowserRouterMode,
    UnifiedBrowserRouterBuilder,
};
use gateway_core::llm::providers::openai::{OpenAIConfig, OpenAIProvider};
use gateway_core::llm::{LlmConfig, LlmRouter};
use gateway_core::mcp::McpGateway;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;

/// Test configuration
struct TestConfig {
    qwen_api_key: String,
    qwen_base_url: String,
    qwen_model: String,
    timeout_secs: u64,
    max_turns: u32,
}

impl TestConfig {
    fn from_env() -> Result<Self, String> {
        let qwen_api_key = std::env::var("QWEN_API_KEY")
            .map_err(|_| "QWEN_API_KEY environment variable not set")?;

        Ok(Self {
            qwen_api_key,
            qwen_base_url: std::env::var("QWEN_BASE_URL").unwrap_or_else(|_| {
                "https://dashscope-intl.aliyuncs.com/compatible-mode".to_string()
            }),
            qwen_model: std::env::var("QWEN_DEFAULT_MODEL")
                .unwrap_or_else(|_| "qwen3-max-2026-01-23".to_string()),
            timeout_secs: std::env::var("TEST_TIMEOUT_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(300),
            max_turns: std::env::var("TEST_MAX_TURNS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(50),
        })
    }
}

/// Create a configured AgentFactory for testing
async fn create_test_agent_factory() -> Result<Arc<AgentFactory>, String> {
    let config = TestConfig::from_env()?;

    let llm_config = LlmConfig::default();
    let mut llm_router = LlmRouter::new(llm_config);

    let qwen_config = OpenAIConfig {
        api_key: config.qwen_api_key,
        base_url: config.qwen_base_url,
        default_model: config.qwen_model,
        organization: None,
        name: "qwen".to_string(),
    };
    llm_router.register_provider("qwen", Arc::new(OpenAIProvider::with_config(qwen_config)));
    llm_router.set_default_provider("qwen");

    let mut ext_config = ExtensionManagerConfig::default();
    ext_config.command_timeout_ms = 180_000;
    let extension_manager = Arc::new(ExtensionManager::with_config(ext_config));
    extension_manager.start_heartbeat_monitor().await;

    let browser_router = UnifiedBrowserRouterBuilder::new()
        .local(extension_manager.clone() as Arc<dyn gateway_core::browser::BrowserRouter>)
        .mode(BrowserRouterMode::LocalOnly)
        .fallback_enabled(false)
        .build();

    let mcp_gateway = Arc::new(McpGateway::new());

    let factory = AgentFactory::new(Arc::new(llm_router))
        .with_mcp_gateway(mcp_gateway)
        .with_browser_router(Arc::new(browser_router))
        .with_max_turns(config.max_turns)
        .with_permission_mode(PermissionMode::BypassPermissions);

    Ok(Arc::new(factory))
}

#[derive(Debug, Default)]
struct TestResult {
    tool_calls: Vec<ToolCallRecord>,
    final_message: Option<String>,
    success: bool,
    error: Option<String>,
    turns: u32,
}

#[derive(Debug, Clone)]
struct ToolCallRecord {
    name: String,
    input: serde_json::Value,
    output: Option<serde_json::Value>,
}

async fn execute_agent_query(
    factory: &AgentFactory,
    prompt: &str,
    timeout_secs: u64,
) -> TestResult {
    let mut result = TestResult::default();
    let session_id = uuid::Uuid::new_v4().to_string();
    let agent = factory.get_or_create(&session_id).await;

    let query_result = timeout(Duration::from_secs(timeout_secs), async {
        let mut agent_guard = agent.write().await;
        let stream = agent_guard.query(prompt).await;
        futures::pin_mut!(stream);

        let mut current_tool_call: Option<ToolCallRecord> = None;

        while let Some(msg_result) = stream.next().await {
            match msg_result {
                Ok(msg) => match msg {
                    AgentMessage::Assistant(assistant) => {
                        for block in &assistant.content {
                            match block {
                                ContentBlock::Text { text } => {
                                    result.final_message = Some(text.clone());
                                }
                                ContentBlock::ToolUse { id: _, name, input } => {
                                    current_tool_call = Some(ToolCallRecord {
                                        name: name.clone(),
                                        input: input.clone(),
                                        output: None,
                                    });
                                }
                                _ => {}
                            }
                        }
                    }
                    AgentMessage::StreamEvent(event) => {
                        if let StreamEventSubtype::ToolResult = event.subtype {
                            if let Some(ref mut call) = current_tool_call {
                                call.output = Some(event.data.clone());
                                result.tool_calls.push(call.clone());
                            }
                            current_tool_call = None;
                        }
                    }
                    AgentMessage::Result(res) => {
                        result.success = !res.is_error;
                        result.turns = res.num_turns;
                    }
                    _ => {}
                },
                Err(e) => {
                    result.error = Some(format!("Stream error: {}", e));
                    break;
                }
            }
        }
    })
    .await;

    if query_result.is_err() {
        result.error = Some("Test timed out".to_string());
    }

    result
}

async fn wait_for_extension_connection(_factory: &AgentFactory, timeout_secs: u64) -> bool {
    let start = std::time::Instant::now();
    loop {
        if start.elapsed().as_secs() >= timeout_secs {
            return false;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
        if start.elapsed().as_secs() >= 5 {
            return true;
        }
    }
}

fn print_test_result(test_id: &str, result: &TestResult) {
    println!("\n========== {} ==========", test_id);
    println!(
        "Status: {}",
        if result.success {
            "✅ PASS"
        } else {
            "❌ FAIL"
        }
    );
    println!("Turns: {}", result.turns);
    println!("Tool calls: {}", result.tool_calls.len());
    for (i, call) in result.tool_calls.iter().enumerate() {
        println!("  {}. {}", i + 1, call.name);
    }
    if let Some(ref msg) = result.final_message {
        println!(
            "Response: {}",
            if msg.len() > 200 { &msg[..200] } else { msg }
        );
    }
    if let Some(ref err) = result.error {
        println!("Error: {}", err);
    }
    println!("==========================================\n");
}

// ============================================================================
// HR-01: Employee Onboarding Email
// ============================================================================

#[tokio::test]
#[ignore = "Requires Chrome extension and Qwen API key"]
async fn test_hr01_employee_onboarding_email() {
    let _ = tracing_subscriber::fmt().with_env_filter("info").try_init();
    let factory = create_test_agent_factory()
        .await
        .expect("Failed to create factory");
    if !wait_for_extension_connection(&factory, 30).await {
        panic!("Chrome extension not connected");
    }

    let prompt = r#"
作为HR，请帮我发送一封新员工入职欢迎邮件：
- 收件人: annie2667@gmail.com
- 主题: 欢迎加入Acme Corp - 入职指南
- 内容:
  亲爱的新同事，

  欢迎加入Acme Team！我们很高兴您能成为我们的一员。

  入职须知：
  1. 请于下周一上午9:00到公司前台报到
  2. 携带身份证原件和学历证书
  3. IT部门会为您配置工作电脑和邮箱账号

  如有任何问题，请随时联系HR部门。

  祝工作愉快！
  HR团队
"#;

    let result = execute_agent_query(&factory, prompt, 300).await;
    print_test_result("HR-01: Employee Onboarding Email", &result);
    assert!(result.success, "HR onboarding email should be sent");
}

// ============================================================================
// HR-02: Create Employee Directory Spreadsheet
// ============================================================================

#[tokio::test]
#[ignore = "Requires Chrome extension and Qwen API key"]
async fn test_hr02_employee_directory() {
    let _ = tracing_subscriber::fmt().with_env_filter("info").try_init();
    let factory = create_test_agent_factory()
        .await
        .expect("Failed to create factory");
    if !wait_for_extension_connection(&factory, 30).await {
        panic!("Chrome extension not connected");
    }

    let prompt = r#"
帮我创建一个员工通讯录的Google表格：
- 表格标题：Acme员工通讯录-2026年Q1
- 表头（第一行）：员工编号 | 姓名 | 部门 | 职位 | 邮箱 | 电话 | 入职日期
- 填写以下员工信息：
  - E001 | 张三 | 技术部 | 高级工程师 | alice@example.com | 10000000001 | 2024-03-15
  - E002 | 李四 | 产品部 | 产品经理 | bob@example.com | 10000000002 | 2024-06-01
  - E003 | 王五 | 市场部 | 市场专员 | charlie@example.com | 10000000003 | 2025-01-10
"#;

    let result = execute_agent_query(&factory, prompt, 300).await;
    print_test_result("HR-02: Employee Directory Spreadsheet", &result);
    assert!(result.success, "Employee directory should be created");
}

// ============================================================================
// HR-03: Schedule Interview Meeting
// ============================================================================

#[tokio::test]
#[ignore = "Requires Chrome extension and Qwen API key"]
async fn test_hr03_schedule_interview() {
    let _ = tracing_subscriber::fmt().with_env_filter("info").try_init();
    let factory = create_test_agent_factory()
        .await
        .expect("Failed to create factory");
    if !wait_for_extension_connection(&factory, 30).await {
        panic!("Chrome extension not connected");
    }

    let prompt = r#"
帮我在Google日历创建一个面试日程：
- 标题：技术岗位面试 - 候选人王小明
- 时间：明天下午2:00-3:00
- 地点：会议室A302
- 描述：
  面试官：张总、李经理
  岗位：高级后端工程师
  简历链接：见邮件附件

  面试流程：
  1. 自我介绍 (10分钟)
  2. 技术问答 (30分钟)
  3. 项目经验 (15分钟)
  4. Q&A (5分钟)
"#;

    let result = execute_agent_query(&factory, prompt, 300).await;
    print_test_result("HR-03: Schedule Interview", &result);
    assert!(result.success, "Interview should be scheduled");
}

// ============================================================================
// PM-01: Create Project Kickoff Document
// ============================================================================

#[tokio::test]
#[ignore = "Requires Chrome extension and Qwen API key"]
async fn test_pm01_project_kickoff_doc() {
    let _ = tracing_subscriber::fmt().with_env_filter("info").try_init();
    let factory = create_test_agent_factory()
        .await
        .expect("Failed to create factory");
    if !wait_for_extension_connection(&factory, 30).await {
        panic!("Chrome extension not connected");
    }

    let prompt = r#"
帮我创建一个项目启动文档(Google Doc)：
- 标题：Project Phoenix - 项目启动文档
- 内容：

# Project Phoenix 项目启动文档

## 1. 项目概述
本项目旨在重构公司核心交易系统，提升系统性能和可维护性。

## 2. 项目目标
- 系统响应时间降低50%
- 代码覆盖率提升至80%
- 支持10倍并发量

## 3. 项目团队
- 项目经理：张三
- 技术负责人：李四
- 开发团队：5人
- 测试团队：2人

## 4. 里程碑
| 阶段 | 时间 | 交付物 |
|------|------|--------|
| 需求分析 | W1-W2 | 需求文档 |
| 系统设计 | W3-W4 | 设计文档 |
| 开发阶段 | W5-W12 | 代码 |
| 测试阶段 | W13-W14 | 测试报告 |
| 上线部署 | W15 | 生产环境 |

## 5. 风险识别
- 技术风险：新技术栈学习曲线
- 资源风险：核心开发人员离职
- 进度风险：需求变更频繁
"#;

    let result = execute_agent_query(&factory, prompt, 300).await;
    print_test_result("PM-01: Project Kickoff Document", &result);
    assert!(result.success, "Project document should be created");
}

// ============================================================================
// PM-02: Project Task Tracking Sheet
// ============================================================================

#[tokio::test]
#[ignore = "Requires Chrome extension and Qwen API key"]
async fn test_pm02_task_tracking_sheet() {
    let _ = tracing_subscriber::fmt().with_env_filter("info").try_init();
    let factory = create_test_agent_factory()
        .await
        .expect("Failed to create factory");
    if !wait_for_extension_connection(&factory, 30).await {
        panic!("Chrome extension not connected");
    }

    let prompt = r#"
创建一个项目任务跟踪表格(Google Sheets)：
- 标题：Project Phoenix - 任务跟踪表
- 表头：任务ID | 任务名称 | 负责人 | 状态 | 优先级 | 开始日期 | 截止日期 | 备注
- 添加以下任务：
  - T001 | 需求评审会议 | 张三 | 已完成 | P0 | 2026-01-15 | 2026-01-15 | -
  - T002 | 数据库设计 | 李四 | 进行中 | P0 | 2026-01-20 | 2026-01-25 | 待评审
  - T003 | API接口定义 | 王五 | 进行中 | P1 | 2026-01-22 | 2026-01-28 | -
  - T004 | 前端原型设计 | 赵六 | 待开始 | P1 | 2026-01-25 | 2026-02-01 | -
  - T005 | 核心模块开发 | 李四 | 待开始 | P0 | 2026-01-28 | 2026-02-15 | 依赖T002
"#;

    let result = execute_agent_query(&factory, prompt, 300).await;
    print_test_result("PM-02: Task Tracking Sheet", &result);
    assert!(result.success, "Task tracking sheet should be created");
}

// ============================================================================
// PM-03: Weekly Status Meeting
// ============================================================================

#[tokio::test]
#[ignore = "Requires Chrome extension and Qwen API key"]
async fn test_pm03_weekly_status_meeting() {
    let _ = tracing_subscriber::fmt().with_env_filter("info").try_init();
    let factory = create_test_agent_factory()
        .await
        .expect("Failed to create factory");
    if !wait_for_extension_connection(&factory, 30).await {
        panic!("Chrome extension not connected");
    }

    let prompt = r#"
在Google日历创建一个每周例会：
- 标题：Project Phoenix 周例会
- 时间：每周五下午3:00-4:00（先创建本周五的）
- 地点：线上会议 - Zoom
- 描述：
  参会人员：项目全体成员

  会议议程：
  1. 本周工作回顾 (15分钟)
  2. 问题讨论与解决 (20分钟)
  3. 下周工作计划 (15分钟)
  4. 其他事项 (10分钟)

  请各位提前准备好本周工作汇报。
"#;

    let result = execute_agent_query(&factory, prompt, 300).await;
    print_test_result("PM-03: Weekly Status Meeting", &result);
    assert!(result.success, "Weekly meeting should be created");
}

// ============================================================================
// FIN-01: Monthly Expense Report
// ============================================================================

#[tokio::test]
#[ignore = "Requires Chrome extension and Qwen API key"]
async fn test_fin01_expense_report() {
    let _ = tracing_subscriber::fmt().with_env_filter("info").try_init();
    let factory = create_test_agent_factory()
        .await
        .expect("Failed to create factory");
    if !wait_for_extension_connection(&factory, 30).await {
        panic!("Chrome extension not connected");
    }

    let prompt = r#"
创建一个部门月度费用报表(Google Sheets)：
- 标题：技术部2026年1月费用报表
- Sheet1标题：费用明细
- 表头：日期 | 费用类型 | 金额(元) | 用途说明 | 报销人 | 审批状态
- 添加数据：
  - 2026-01-05 | 办公用品 | 2,580 | 购买显示器支架10个 | 张三 | 已审批
  - 2026-01-10 | 差旅费用 | 5,200 | 上海出差往返机票+酒店 | 李四 | 已审批
  - 2026-01-15 | 培训费用 | 8,000 | AWS云计算培训 | 王五 | 待审批
  - 2026-01-20 | 软件订阅 | 12,000 | JetBrains全家桶年费 | 赵六 | 已审批
  - 2026-01-25 | 团建费用 | 3,500 | 部门聚餐 | 张三 | 待审批
- 在最后添加一行汇总：合计 | - | =SUM(C2:C6) | - | - | -
"#;

    let result = execute_agent_query(&factory, prompt, 300).await;
    print_test_result("FIN-01: Monthly Expense Report", &result);
    assert!(result.success, "Expense report should be created");
}

// ============================================================================
// FIN-02: Invoice Email to Client
// ============================================================================

#[tokio::test]
#[ignore = "Requires Chrome extension and Qwen API key"]
async fn test_fin02_invoice_email() {
    let _ = tracing_subscriber::fmt().with_env_filter("info").try_init();
    let factory = create_test_agent_factory()
        .await
        .expect("Failed to create factory");
    if !wait_for_extension_connection(&factory, 30).await {
        panic!("Chrome extension not connected");
    }

    let prompt = r#"
发送一封发票邮件给客户：
- 收件人: annie2667@gmail.com
- 主题: Acme Corp - 2026年1月服务费发票
- 内容:
  尊敬的客户，

  感谢您选择Acme Corp的服务。

  现附上2026年1月的服务费用发票，详情如下：

  发票编号：INV-2026-0128
  开票日期：2026年1月28日
  服务期间：2026年1月1日 - 2026年1月31日

  费用明细：
  - 系统维护服务费：¥15,000.00
  - 技术支持服务费：¥8,000.00
  - 小计：¥23,000.00
  - 税额(6%)：¥1,380.00
  - 合计：¥24,380.00

  付款方式：银行转账
  收款账户：Acme Corp
  开户行：中国银行XX支行
  账号：1234 5678 9012 3456

  请于收到发票后30天内完成付款。如有疑问请随时联系我们。

  财务部
  Acme Corp
"#;

    let result = execute_agent_query(&factory, prompt, 300).await;
    print_test_result("FIN-02: Invoice Email", &result);
    assert!(result.success, "Invoice email should be sent");
}

// ============================================================================
// CRM-01: Customer Follow-up Email
// ============================================================================

#[tokio::test]
#[ignore = "Requires Chrome extension and Qwen API key"]
async fn test_crm01_customer_followup() {
    let _ = tracing_subscriber::fmt().with_env_filter("info").try_init();
    let factory = create_test_agent_factory()
        .await
        .expect("Failed to create factory");
    if !wait_for_extension_connection(&factory, 30).await {
        panic!("Chrome extension not connected");
    }

    let prompt = r#"
发送客户跟进邮件：
- 收件人: annie2667@gmail.com
- 主题: 关于上周产品演示的后续沟通
- 内容:
  尊敬的王总，

  您好！非常感谢您上周抽出宝贵时间参加我们的产品演示会。

  根据您在会上提出的几点需求，我们团队进行了深入讨论：

  1. 关于数据导入功能
     我们可以支持Excel、CSV格式的批量导入，并提供数据校验功能。

  2. 关于权限管理
     系统支持多级角色权限配置，可根据贵公司组织架构灵活设置。

  3. 关于报价方案
     附件中包含我们为贵公司量身定制的报价方案，包含三种套餐供您选择。

  如果您方便的话，我们可以安排一次更深入的技术交流会议。
  请问本周三或周四下午您是否有空？

  期待您的回复！

  销售经理 李明
  Acme Corp
  电话：139-xxxx-xxxx
"#;

    let result = execute_agent_query(&factory, prompt, 300).await;
    print_test_result("CRM-01: Customer Follow-up Email", &result);
    assert!(result.success, "Follow-up email should be sent");
}

// ============================================================================
// CRM-02: Customer Database Sheet
// ============================================================================

#[tokio::test]
#[ignore = "Requires Chrome extension and Qwen API key"]
async fn test_crm02_customer_database() {
    let _ = tracing_subscriber::fmt().with_env_filter("info").try_init();
    let factory = create_test_agent_factory()
        .await
        .expect("Failed to create factory");
    if !wait_for_extension_connection(&factory, 30).await {
        panic!("Chrome extension not connected");
    }

    let prompt = r#"
创建客户信息管理表格(Google Sheets)：
- 标题：2026年Q1客户信息库
- 表头：客户编号 | 公司名称 | 联系人 | 职位 | 电话 | 邮箱 | 行业 | 客户等级 | 跟进状态 | 最近联系日期
- 添加以下客户：
  - C001 | 华为技术有限公司 | 张经理 | 采购总监 | 13800001111 | zhang@huawei.com | 科技 | A | 已成交 | 2026-01-25
  - C002 | 阿里巴巴集团 | 李总 | VP | 13800002222 | li@alibaba.com | 互联网 | A | 商务谈判中 | 2026-01-28
  - C003 | 中国银行 | 王主任 | IT部主任 | 13800003333 | wang@boc.cn | 金融 | B | 需求确认中 | 2026-01-20
  - C004 | 比亚迪汽车 | 赵总监 | 信息化总监 | 13800004444 | zhao@byd.com | 制造 | B | 初次接触 | 2026-01-26
  - C005 | 顺丰速运 | 孙经理 | 技术经理 | 13800005555 | sun@sf.com | 物流 | C | 待跟进 | 2026-01-15
"#;

    let result = execute_agent_query(&factory, prompt, 300).await;
    print_test_result("CRM-02: Customer Database Sheet", &result);
    assert!(result.success, "Customer database should be created");
}

// ============================================================================
// COM-01: Company Announcement Email
// ============================================================================

#[tokio::test]
#[ignore = "Requires Chrome extension and Qwen API key"]
async fn test_com01_company_announcement() {
    let _ = tracing_subscriber::fmt().with_env_filter("info").try_init();
    let factory = create_test_agent_factory()
        .await
        .expect("Failed to create factory");
    if !wait_for_extension_connection(&factory, 30).await {
        panic!("Chrome extension not connected");
    }

    let prompt = r#"
发送公司公告邮件：
- 收件人: annie2667@gmail.com
- 主题: 【重要通知】关于2026年春节放假安排
- 内容:
  全体员工：

  根据国家法定节假日安排，现将公司2026年春节放假安排通知如下：

  一、放假时间
  2026年1月26日（除夕）至2026年2月4日（正月初九），共10天。
  2026年2月5日（正月初十）正常上班。

  二、值班安排
  节日期间，各部门需安排值班人员，确保紧急事务处理：
  - 技术部值班：张三 (1月26-28日)、李四 (1月29-31日)
  - 客服部值班：王五 (全程)

  三、注意事项
  1. 离开前请确保办公电脑关闭，贵重物品妥善保管
  2. 外出期间注意人身和财产安全
  3. 如有紧急工作事项，请联系值班人员

  祝大家新春快乐，阖家幸福！

  行政人事部
  2026年1月28日
"#;

    let result = execute_agent_query(&factory, prompt, 300).await;
    print_test_result("COM-01: Company Announcement", &result);
    assert!(result.success, "Announcement email should be sent");
}

// ============================================================================
// COM-02: Meeting Minutes Document
// ============================================================================

#[tokio::test]
#[ignore = "Requires Chrome extension and Qwen API key"]
async fn test_com02_meeting_minutes() {
    let _ = tracing_subscriber::fmt().with_env_filter("info").try_init();
    let factory = create_test_agent_factory()
        .await
        .expect("Failed to create factory");
    if !wait_for_extension_connection(&factory, 30).await {
        panic!("Chrome extension not connected");
    }

    let prompt = r#"
创建会议纪要文档(Google Doc)：
- 标题：2026年度战略规划会议纪要

# 2026年度战略规划会议纪要

## 会议信息
- 时间：2026年1月28日 14:00-17:00
- 地点：公司大会议室
- 主持人：CEO 陈总
- 参会人：各部门负责人（共12人）
- 记录人：行政助理 小王

## 会议议程

### 1. 2025年度工作总结
- 全年营收：1.2亿元，同比增长35%
- 新增客户：86家
- 团队规模：从45人扩展到78人
- 产品迭代：发布3个大版本

### 2. 2026年战略目标
- 营收目标：2亿元
- 市场份额：从5%提升到8%
- 团队目标：年底达到120人
- 产品目标：推出SaaS版本

### 3. 重点工作部署
1. Q1：完成产品SaaS化改造
2. Q2：拓展华东市场
3. Q3：启动海外市场调研
4. Q4：完成B轮融资

### 4. 待办事项
| 事项 | 负责人 | 截止日期 |
|------|--------|----------|
| 制定部门年度计划 | 各部门负责人 | 2026-02-10 |
| SaaS产品立项报告 | 产品部 张总 | 2026-02-05 |
| 招聘计划制定 | HR 李总 | 2026-02-08 |

## 下次会议
时间：2026年2月25日 14:00
主题：Q1工作进展评估
"#;

    let result = execute_agent_query(&factory, prompt, 300).await;
    print_test_result("COM-02: Meeting Minutes", &result);
    assert!(result.success, "Meeting minutes should be created");
}

// ============================================================================
// REC-01: Job Posting Research
// ============================================================================

#[tokio::test]
#[ignore = "Requires Chrome extension and Qwen API key"]
async fn test_rec01_job_posting_research() {
    let _ = tracing_subscriber::fmt().with_env_filter("info").try_init();
    let factory = create_test_agent_factory()
        .await
        .expect("Failed to create factory");
    if !wait_for_extension_connection(&factory, 30).await {
        panic!("Chrome extension not connected");
    }

    let prompt = r#"
帮我搜索一下目前市场上"高级Rust工程师"的招聘要求和薪资范围。
请访问一个招聘网站（如智联招聘、Boss直聘或LinkedIn），
搜索相关职位，告诉我：
1. 常见的任职要求有哪些
2. 薪资范围大概是多少
3. 需要哪些核心技能
"#;

    let result = execute_agent_query(&factory, prompt, 300).await;
    print_test_result("REC-01: Job Posting Research", &result);
    assert!(result.success, "Job research should complete");
}

// ============================================================================
// REC-02: Interview Invitation Email
// ============================================================================

#[tokio::test]
#[ignore = "Requires Chrome extension and Qwen API key"]
async fn test_rec02_interview_invitation() {
    let _ = tracing_subscriber::fmt().with_env_filter("info").try_init();
    let factory = create_test_agent_factory()
        .await
        .expect("Failed to create factory");
    if !wait_for_extension_connection(&factory, 30).await {
        panic!("Chrome extension not connected");
    }

    let prompt = r#"
发送面试邀请邮件：
- 收件人: annie2667@gmail.com
- 主题: Acme Corp - 高级后端工程师面试邀请
- 内容:
  尊敬的候选人，

  您好！非常感谢您对Acme Corp的关注和投递。

  经过简历初筛，我们对您的背景和经验非常感兴趣，
  现诚邀您参加我们的面试。

  面试安排：
  - 日期：2026年2月3日（周一）
  - 时间：下午2:00 - 3:30
  - 地点：上海市浦东新区张江高科技园区XX大厦15楼
  - 联系人：HR 小李，电话 139-xxxx-xxxx

  面试流程：
  1. 技术面试（45分钟）- 由技术总监主持
  2. 项目经验交流（30分钟）- 由团队Leader主持
  3. HR面谈（15分钟）- 薪资福利沟通

  请携带：
  - 身份证原件
  - 学历证书原件
  - 作品集或项目介绍（如有）

  如有任何时间冲突，请提前与我们联系协调。

  期待与您见面！

  Acme Corp 人力资源部
"#;

    let result = execute_agent_query(&factory, prompt, 300).await;
    print_test_result("REC-02: Interview Invitation", &result);
    assert!(result.success, "Interview invitation should be sent");
}

// ============================================================================
// REC-03: Candidate Tracking Sheet
// ============================================================================

#[tokio::test]
#[ignore = "Requires Chrome extension and Qwen API key"]
async fn test_rec03_candidate_tracking() {
    let _ = tracing_subscriber::fmt().with_env_filter("info").try_init();
    let factory = create_test_agent_factory()
        .await
        .expect("Failed to create factory");
    if !wait_for_extension_connection(&factory, 30).await {
        panic!("Chrome extension not connected");
    }

    let prompt = r#"
创建候选人追踪表格(Google Sheets)：
- 标题：2026年Q1招聘候选人追踪表
- 表头：候选人姓名 | 应聘职位 | 简历来源 | 投递日期 | 当前状态 | 面试官 | 面试评分 | 期望薪资 | 备注
- 添加以下候选人：
  - 张小明 | 高级后端工程师 | Boss直聘 | 2026-01-15 | 待面试 | - | - | 35K | Rust经验3年
  - 李小红 | 前端开发工程师 | 内推 | 2026-01-18 | 一面通过 | 王总监 | 4.2 | 28K | Vue/React均可
  - 王大力 | 产品经理 | 智联招聘 | 2026-01-20 | 二面中 | 陈总 | 4.5 | 40K | ToB产品经验丰富
  - 赵小云 | UI设计师 | LinkedIn | 2026-01-22 | 已发offer | HR | 4.0 | 25K | 作品集优秀
  - 孙小海 | 测试工程师 | 校招 | 2026-01-25 | 简历筛选 | - | - | 15K | 应届硕士
"#;

    let result = execute_agent_query(&factory, prompt, 300).await;
    print_test_result("REC-03: Candidate Tracking Sheet", &result);
    assert!(result.success, "Candidate tracking sheet should be created");
}

// ============================================================================
// OPS-01: Server Monitoring Alert Email
// ============================================================================

#[tokio::test]
#[ignore = "Requires Chrome extension and Qwen API key"]
async fn test_ops01_monitoring_alert() {
    let _ = tracing_subscriber::fmt().with_env_filter("info").try_init();
    let factory = create_test_agent_factory()
        .await
        .expect("Failed to create factory");
    if !wait_for_extension_connection(&factory, 30).await {
        panic!("Chrome extension not connected");
    }

    let prompt = r#"
发送运维告警通知邮件：
- 收件人: annie2667@gmail.com
- 主题: 【告警】生产环境数据库CPU使用率过高
- 内容:
  运维团队 各位：

  监控系统检测到生产环境异常，请及时处理。

  ⚠️ 告警信息
  ---------------------
  告警级别：Warning
  告警时间：2026-01-28 15:32:18
  服务器：prod-db-master-01
  告警指标：CPU使用率
  当前值：87.5%
  阈值：80%
  持续时间：15分钟

  📊 相关指标
  ---------------------
  - 内存使用率：72.3%
  - 磁盘I/O：中等
  - 网络流量：正常
  - 活跃连接数：1,234

  🔧 建议处理措施
  ---------------------
  1. 检查是否有慢查询导致CPU飙高
  2. 查看当前活跃连接的SQL语句
  3. 如持续过高，考虑临时切换至从库
  4. 联系DBA进行SQL优化

  📞 值班人员
  ---------------------
  今日值班：张三 (139-xxxx-xxxx)
  DBA值班：李四 (138-xxxx-xxxx)

  此邮件由监控系统自动发送，请勿直接回复。

  运维监控系统
"#;

    let result = execute_agent_query(&factory, prompt, 300).await;
    print_test_result("OPS-01: Monitoring Alert Email", &result);
    assert!(result.success, "Alert email should be sent");
}

// ============================================================================
// OPS-02: Deployment Schedule
// ============================================================================

#[tokio::test]
#[ignore = "Requires Chrome extension and Qwen API key"]
async fn test_ops02_deployment_schedule() {
    let _ = tracing_subscriber::fmt().with_env_filter("info").try_init();
    let factory = create_test_agent_factory()
        .await
        .expect("Failed to create factory");
    if !wait_for_extension_connection(&factory, 30).await {
        panic!("Chrome extension not connected");
    }

    let prompt = r#"
在Google日历创建一个生产环境发版日程：
- 标题：🚀 v2.5.0 生产环境发版
- 时间：本周六凌晨2:00-4:00
- 描述：
  发版计划 v2.5.0
  ==================

  📋 发版内容：
  1. 用户中心模块优化
  2. 支付系统Bug修复
  3. 性能优化（预计提升20%）

  👥 参与人员：
  - 发版负责人：张三
  - 后端支持：李四
  - 前端支持：王五
  - DBA支持：赵六（待命）

  📝 发版流程：
  02:00 - 切换流量至灰度环境
  02:15 - 备份数据库
  02:30 - 部署后端服务
  02:45 - 部署前端资源
  03:00 - 冒烟测试
  03:30 - 全量切流
  03:45 - 监控观察
  04:00 - 发版完成/回滚决策

  ⚠️ 回滚方案：
  如发现严重问题，立即执行回滚脚本
  回滚时间预估：15分钟

  📞 紧急联系人：
  运维经理：138-xxxx-xxxx
"#;

    let result = execute_agent_query(&factory, prompt, 300).await;
    print_test_result("OPS-02: Deployment Schedule", &result);
    assert!(result.success, "Deployment schedule should be created");
}

// ============================================================================
// Batch Test: All Enterprise Scenarios
// ============================================================================

#[tokio::test]
#[ignore = "Requires Chrome extension and Qwen API key"]
async fn test_enterprise_batch() {
    let _ = tracing_subscriber::fmt().with_env_filter("info").try_init();
    let factory = create_test_agent_factory()
        .await
        .expect("Failed to create factory");
    if !wait_for_extension_connection(&factory, 30).await {
        panic!("Chrome extension not connected");
    }

    let test_cases = vec![
        (
            "HR-01",
            "发送入职欢迎邮件给annie2667@gmail.com，主题'欢迎加入'，内容'欢迎新同事加入团队'",
        ),
        (
            "PM-01",
            "创建Google文档，标题'项目周报'，内容'本周完成任务列表'",
        ),
        (
            "FIN-01",
            "创建Google表格，标题'费用报表'，添加表头：日期|类型|金额",
        ),
        (
            "CRM-01",
            "发送客户跟进邮件给annie2667@gmail.com，主题'合作洽谈'",
        ),
    ];

    let mut results = vec![];

    for (test_id, prompt) in test_cases {
        println!("\n>>> Running: {} <<<", test_id);
        let result = execute_agent_query(&factory, prompt, 180).await;
        let passed = result.success;
        println!(
            "{}: {}",
            test_id,
            if passed { "✅ PASS" } else { "❌ FAIL" }
        );
        results.push((test_id, passed));
        tokio::time::sleep(Duration::from_secs(5)).await;
    }

    println!("\n========== Enterprise Batch Summary ==========");
    let mut pass_count = 0;
    for (test_id, passed) in &results {
        println!("{}: {}", test_id, if *passed { "✅" } else { "❌" });
        if *passed {
            pass_count += 1;
        }
    }
    println!("Total: {}/{} passed", pass_count, results.len());
    println!("==============================================\n");
}
