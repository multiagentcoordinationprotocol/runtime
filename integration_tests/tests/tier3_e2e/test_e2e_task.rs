use crate::common;
use macp_integration_tests::helpers::*;
use macp_integration_tests::macp_tools::{self, task::*};
use rig::completion::Prompt;
use rig::providers::openai;
use rig::prelude::*;

/// Realistic task delegation: planner (plain code) delegates to worker (LLM).
///
/// Architecture:
///   - Planner: plain Rust code (deterministic session/task/commit operations)
///   - Worker: real GPT-4o-mini (LLM reasoning to accept and produce analysis)
///
/// The LLM is used ONLY where reasoning adds value — the worker deciding how
/// to complete the task and producing the analysis output.
#[tokio::test]
#[ignore]
async fn real_llm_agents_delegate_task() {
    if std::env::var("OPENAI_API_KEY").is_err() {
        eprintln!("SKIPPED: OPENAI_API_KEY not set");
        return;
    }

    let ep = common::endpoint().await;
    let sid = new_session_id();
    let planner_id = "agent://planner";
    let worker_id = "agent://data-analyst";

    eprintln!("═══════════════════════════════════════════════════════════════");
    eprintln!("  MACP Task Mode E2E Test — Q4 Data Analysis Delegation");
    eprintln!("  Session: {sid}");
    eprintln!("  Planner: plain code (no LLM)");
    eprintln!("  Worker:  GPT-4o-mini (real LLM reasoning)");
    eprintln!("═══════════════════════════════════════════════════════════════");

    // ── Planner gRPC client (no LLM needed) ─────────────────────────────
    let planner_client = macp_tools::shared_client(ep).await;

    // ═══════════════════════════════════════════════════════════════════
    // STEP 1: Planner starts session (plain code)
    // ═══════════════════════════════════════════════════════════════════
    eprintln!("\n── Step 1: Planner starts session (deterministic) ───────────");
    {
        let mut client = planner_client.lock().await;
        let ack = send_as(
            &mut client,
            planner_id,
            envelope(
                MODE_TASK,
                "SessionStart",
                &new_message_id(),
                &sid,
                planner_id,
                session_start_payload(
                    "Q4 revenue analysis delegation",
                    &[planner_id, worker_id],
                    60_000,
                ),
            ),
        )
        .await
        .expect("SessionStart should succeed");
        assert!(ack.ok);
        eprintln!("   Session started. State: OPEN");
    }

    // ═══════════════════════════════════════════════════════════════════
    // STEP 2: Planner creates task (plain code)
    // ═══════════════════════════════════════════════════════════════════
    eprintln!("\n── Step 2: Planner creates task request (deterministic) ─────");
    {
        let mut client = planner_client.lock().await;
        let ack = send_as(
            &mut client,
            planner_id,
            envelope(
                MODE_TASK,
                "TaskRequest",
                &new_message_id(),
                &sid,
                planner_id,
                task_request_payload(
                    "q4-analysis",
                    "Q4 Revenue Data Analysis",
                    "Analyze Q4 revenue by region and identify top growth drivers. Include YoY comparison and highlight any anomalies.",
                    worker_id,
                ),
            ),
        )
        .await
        .expect("TaskRequest should succeed");
        assert!(ack.ok);
        eprintln!("   Task created: \"Q4 Revenue Data Analysis\"");
        eprintln!("   Assigned to: {worker_id}");
    }

    // ═══════════════════════════════════════════════════════════════════
    // STEP 3: Worker accepts and completes (REAL LLM REASONING)
    // ═══════════════════════════════════════════════════════════════════
    eprintln!("\n── Step 3: Worker accepts and completes task (LLM) ──────────");
    eprintln!("   Worker uses GPT-4o-mini to reason about the task\n");

    let openai_client = openai::Client::from_env();
    let worker_client = macp_tools::shared_client(ep).await;
    let worker_agent = openai_client
        .agent(openai::GPT_4O_MINI)
        .preamble(
            "You are a data analyst agent. When assigned a task, you accept it using \
             the macp_task_accept tool, then complete it using the macp_task_complete tool \
             with a professional analysis summary.\n\n\
             Use the EXACT session_id and task_id values from the prompt.\n\
             For the summary in macp_task_complete, write a realistic data analysis finding.",
        )
        .tool(TaskAcceptTool {
            client: worker_client.clone(),
            agent_id: worker_id.into(),
        })
        .tool(TaskCompleteTool {
            client: worker_client.clone(),
            agent_id: worker_id.into(),
        })
        .default_max_turns(10)
        .build();

    eprintln!("   [Data Analyst] Accepting and analyzing with GPT-4o-mini...");
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        worker_agent.prompt(&format!(
            "You have been assigned a task: \"Q4 Revenue Data Analysis\"\n\
             Instructions: Analyze Q4 revenue by region and identify top growth drivers. \
             Include YoY comparison and highlight any anomalies.\n\n\
             First accept the task, then complete it with your analysis.\n\
             - session_id: {sid}\n\
             - task_id: \"q4-analysis\"\n\
             For the completion summary, write realistic revenue analysis findings."
        )),
    )
    .await;
    match &result {
        Ok(Ok(text)) => eprintln!("   Data Analyst response: {text}\n"),
        Ok(Err(e)) => panic!("Worker agent failed: {e}"),
        Err(_) => panic!("Worker agent timed out"),
    }

    // ═══════════════════════════════════════════════════════════════════
    // STEP 4: Planner commits (plain code)
    // ═══════════════════════════════════════════════════════════════════
    eprintln!("── Step 4: Planner commits completed task (deterministic) ───");
    {
        let mut client = planner_client.lock().await;
        let ack = send_as(
            &mut client,
            planner_id,
            envelope(
                MODE_TASK,
                "Commitment",
                &new_message_id(),
                &sid,
                planner_id,
                commitment_payload(
                    "cmt-001",
                    "task-completed",
                    "planner",
                    "Data analyst delivered Q4 revenue analysis with regional breakdown",
                ),
            ),
        )
        .await
        .expect("Commitment should succeed");
        assert!(ack.ok);
        assert_eq!(ack.session_state, 2);
        eprintln!("   Committed. Session state: RESOLVED");
    }

    // ═══════════════════════════════════════════════════════════════════
    // STEP 5: Verify
    // ═══════════════════════════════════════════════════════════════════
    eprintln!("\n── Step 5: Verify session state ─────────────────────────────");
    {
        let mut client = planner_client.lock().await;
        let resp = get_session_as(&mut client, planner_id, &sid)
            .await
            .expect("GetSession should succeed");
        let meta = resp.metadata.expect("metadata present");
        assert_eq!(meta.state, 2);
        eprintln!("   Session state: {} (RESOLVED)", meta.state);
    }

    eprintln!("\n═══════════════════════════════════════════════════════════════");
    eprintln!("  PASSED");
    eprintln!("  Planner (code) created task → Worker (LLM) analyzed and completed");
    eprintln!("  → Planner (code) committed");
    eprintln!("  LLM was used ONLY where reasoning was needed (worker analysis)");
    eprintln!("═══════════════════════════════════════════════════════════════");
}
