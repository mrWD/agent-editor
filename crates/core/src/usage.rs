//! What the agents cost and how close they are to their limits, and handing
//! a task from one agent to another.
//!
//! Agents report, with their answers, how much of the context is in use and
//! what the session has cost so far. The core keeps those notes with the
//! task's events; here they are added up. Nothing is estimated: an agent that
//! reports no cost shows none.

use crate::tasks::{Manager, TaskInfo, TaskState};
use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

#[derive(Default, Clone)]
struct Spent {
    today: f64,
    week: f64,
    total: f64,
}

impl Spent {
    fn add(&mut self, amount: f64, at: u64, day_start: u64, week_start: u64) {
        self.total += amount;
        if at >= week_start {
            self.week += amount;
        }
        if at >= day_start {
            self.today += amount;
        }
    }
}

fn round(amount: f64) -> f64 {
    (amount * 10_000.0).round() / 10_000.0
}

impl Manager {
    /// Cost by agent and by task, and what each agent last said about its limits.
    /// `day_start` and `week_start` are moments in milliseconds: the interface
    /// knows where the user's day begins, the core does not.
    pub fn usage_summary(&self, day_start: u64, week_start: u64) -> Result<Value> {
        let mut by_agent: HashMap<String, Spent> = HashMap::new();
        let mut by_task: BTreeMap<String, (Spent, Vec<String>, String, String, bool)> = BTreeMap::new();
        let mut limits: HashMap<String, (u64, Value)> = HashMap::new();
        let mut currency = "USD".to_string();

        // A session's cost only grows, and a new session counts from zero again.
        // So what was spent is the sum of the steps up, each at its own moment.
        let mut last: HashMap<String, f64> = HashMap::new();
        let mut agent_of: HashMap<String, String> = HashMap::new();
        for row in self.store().usage_events()? {
            let event = &row.body;
            if event["type"] == "agent_started" {
                if let Some(agent) = event["agentId"].as_str() {
                    agent_of.insert(row.task_id.clone(), agent.to_string());
                }
                // An agent that picked its session up may go on counting where it was, or from zero.
                continue;
            }
            let update = &event["update"];
            let agent = agent_of.get(&row.task_id).cloned().unwrap_or_else(|| row.agent_id.clone());
            if let Some(limit) = update.pointer("/_meta/_claude~1rateLimit").filter(|limit| limit.is_object()) {
                limits.insert(agent.clone(), (row.at, limit.clone()));
            }
            let Some(amount) = update.pointer("/cost/amount").and_then(Value::as_f64) else { continue };
            if let Some(name) = update.pointer("/cost/currency").and_then(Value::as_str) {
                currency = name.to_string();
            }
            let before = last.insert(row.task_id.clone(), amount).unwrap_or(0.0);
            let step = if amount >= before { amount - before } else { amount };
            if step <= 0.0 {
                continue;
            }
            by_agent.entry(agent.clone()).or_default().add(step, row.at, day_start, week_start);
            let task = by_task.entry(row.task_id.clone()).or_insert_with(|| {
                (Spent::default(), Vec::new(), row.title.clone(), row.project_id.clone(), row.archived)
            });
            task.0.add(step, row.at, day_start, week_start);
            if !task.1.contains(&agent) {
                task.1.push(agent);
            }
        }

        let live = self.list()?;
        let agents: Vec<Value> = self
            .registry()
            .list()
            .iter()
            .map(|agent| {
                let spent = by_agent.get(&agent.id).cloned().unwrap_or_default();
                let mine: Vec<&TaskInfo> = live.iter().filter(|task| task.row.agent_id == agent.id).collect();
                let working = mine
                    .iter()
                    .filter(|t| matches!(t.state, TaskState::Running | TaskState::WaitingPermission))
                    .count();
                json!({
                    "id": agent.id,
                    "name": agent.name,
                    "short": agent.short,
                    "vendor": agent.vendor,
                    "available": agent.available,
                    "note": agent.note,
                    "spendsTokens": agent.spends_tokens,
                    "tasks": mine.len(),
                    "working": working,
                    "today": round(spent.today),
                    "week": round(spent.week),
                    "total": round(spent.total),
                    // What the agent itself last said about its limits, and when. Shown as it is.
                    "limit": limits.get(&agent.id).map(|(at, limit)| json!({ "at": at, "info": limit })),
                })
            })
            .collect();
        let mut tasks: Vec<Value> = by_task
            .into_iter()
            .filter(|(_, (spent, ..))| spent.today > 0.0)
            .map(|(id, (spent, agents, title, project_id, archived))| {
                json!({ "taskId": id, "title": title, "projectId": project_id, "agents": agents,
                        "today": round(spent.today), "total": round(spent.total), "archived": archived })
            })
            .collect();
        tasks
            .sort_by(|a, b| b["today"].as_f64().partial_cmp(&a["today"].as_f64()).unwrap_or(std::cmp::Ordering::Equal));
        let today: f64 = by_agent.values().map(|spent| spent.today).sum();

        Ok(json!({
            "agents": agents,
            "tasks": tasks,
            "today": round(today),
            "currency": currency,
            "budget": self.budget(),
        }))
    }

    /// The user's daily budget: how much, and whether to ask before a new task once it is spent.
    pub fn budget(&self) -> Value {
        let daily = self.store().setting("budget.daily").ok().flatten().and_then(|text| text.parse::<f64>().ok());
        let ask = self.store().setting("budget.ask").ok().flatten().is_none_or(|text| text != "0");
        json!({ "daily": daily, "ask": ask })
    }

    pub fn set_budget(&self, daily: Option<Option<f64>>, ask: Option<bool>) -> Result<()> {
        if let Some(daily) = daily {
            if daily.is_some_and(|amount| !amount.is_finite() || amount < 0.0) {
                bail!("A budget is a number of dollars, zero or more");
            }
            self.store().set_setting("budget.daily", daily.map(|amount| amount.to_string()).as_deref())?;
        }
        if let Some(ask) = ask {
            self.store().set_setting("budget.ask", Some(if ask { "1" } else { "0" }))?;
        }
        self.broadcast("usage.changed", json!({}));
        Ok(())
    }

    /// Hands a task from its agent to another one. The new agent has none of
    /// the old one's memory, so with the user's next message it is told what
    /// was said so far: this is how agents of different makers share a task.
    pub async fn switch_agent(self: &Arc<Self>, task_id: &str, agent_id: &str) -> Result<TaskInfo> {
        let task = self.store().task(task_id)?;
        if task.agent_id == agent_id {
            return self.task(task_id);
        }
        let Some(agent) = self.registry().get(agent_id).cloned() else { bail!("No such agent: {agent_id}") };
        if !agent.available {
            bail!(
                "{} is not available on this computer{}",
                agent.name,
                agent.note.map(|n| format!(": {n}")).unwrap_or_default()
            );
        }
        if matches!(self.state_of(task_id), TaskState::Running | TaskState::WaitingPermission) {
            bail!("The agent is working. Stop its turn first, then hand the task over.");
        }
        self.stop_agent_and_wait(task_id).await?;
        self.store().set_task_agent(task_id, agent_id)?;
        let from = self.registry().get(&task.agent_id).map_or(task.agent_id.clone(), |agent| agent.name.clone());
        self.emit(
            task_id,
            json!({ "type": "agent_switched", "from": task.agent_id, "fromName": from, "to": agent.id, "toName": agent.name }),
        );
        let info = self.task(task_id)?;
        self.broadcast("task.updated", json!(info));
        Ok(info)
    }
}
