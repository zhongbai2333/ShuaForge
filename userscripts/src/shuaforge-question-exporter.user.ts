// ==UserScript==
// @name         ShuaForge 题库导出器
// @namespace    https://github.com/ShuaForge
// @version      0.1.0
// @description  从已完成答题/考试结果页提取题目、正确答案、答案解析，并导出为 ShuaForge 可导入的 CSV。只读导出，不自动答题。
// @author       ShuaForge
// @match        *://*.zhihuishu.com/*
// @match        *://*.chaoxing.com/*
// @match        *://*.icve.com.cn/*
// @match        *://*.ai.icve.com.cn/*
// @match        *://*.course.icve.com.cn/*
// @match        *://*.yuketang.cn/*
// @match        *://*.icourse163.org/*
// @match        *://*.webtrn.cn/*
// @match        *://*.courshare.cn/*
// @match        *://*.xueyinonline.com/*
// @match        *://*.edu.cn/*
// @match        *://*.org.cn/*
// @match        *://localhost/*
// @match        *://127.0.0.1/*
// @grant        GM_registerMenuCommand
// @grant        GM_xmlhttpRequest
// @connect      *
// @run-at       document-idle
// ==/UserScript==

declare const GM_registerMenuCommand: undefined | ((caption: string, commandFunc: () => void) => void);
declare const GM_xmlhttpRequest: undefined | ((details: GmXmlHttpRequestDetails) => void);
declare const JSZip: undefined | (new () => ZipArchive);

interface GmXmlHttpRequestDetails {
  method: 'GET';
  url: string;
  responseType: 'blob';
  timeout?: number;
  headers?: Record<string, string>;
  onload(response: GmXmlHttpResponse): void;
  onerror(error: unknown): void;
  ontimeout?(): void;
}

interface GmXmlHttpResponse {
  status: number;
  response: Blob;
  responseHeaders?: string;
}

interface ZipArchive {
  file(path: string, data: Blob | string | Uint8Array): void;
  generateAsync(options: { type: 'blob' }): Promise<Blob>;
}

(function () {
  'use strict';

  interface Problem {
    id: string;
    prompt: string;
    answer: string;
    explanation: string;
    tags: string[];
    images: ProblemImage[];
  }

  interface ProblemImage {
    filename: string;
    mime_type: string;
    base64: string;
    alt_text: string;
    source_url: string;
  }

  interface TextSnapshot {
    text: string;
    images: ImageCandidate[];
  }

  interface ImageCandidate {
    src: string;
    alt: string;
  }

  interface BankInfo {
    name: string;
    info: string;
  }

  interface AnswerScore {
    value: number;
    display: string;
  }

  interface ParsedProblemDraft {
    problem: Problem;
    promptLine: string;
    options: string[];
    answer: string;
    fullText: string;
    answerScore: AnswerScore | null;
  }

  interface VirtualQuestionBlock {
    nodeType: number;
    tagName: string;
    className: string;
    innerText: string;
    textContent: string;
    dataset: { virtualIndex: string };
    querySelectorAll(selectors: string): HTMLElement[];
    compareDocumentPosition(other: QuestionBlock): number;
    contains(other: QuestionBlock): boolean;
  }

  type QuestionBlock = HTMLElement | VirtualQuestionBlock;

  const PANEL_ID = 'shuaforge-exporter-panel';
  const SELECTING_CLASS = 'shuaforge-exporter-selecting';
  const VIRTUAL_BLOCK_CLASS = 'shuaforge-exporter-virtual-block';
  const IMAGE_PLACEHOLDER_PREFIX = '[图片]';
  const JSZIP_CDN = 'https://cdn.jsdelivr.net/npm/jszip@3.10.1/dist/jszip.min.js';
  const PENDING_REVIEW_NOTE = '未批改：页面没有提供正确答案，已使用“我的答案”作为临时答案。';
  const SCORE_INFERRED_NOTE = '页面没有提供标准答案，已使用“我的答案”作为导出答案；本题得分可用于判断该作答是否正确。';
  let selectedRoot: HTMLElement = document.body;
  let selecting = false;
  let lastProblems: Problem[] = [];

  const QUESTION_ROOT_SELECTORS = [
    // 超星/学习通结果页：每道题是 questionLi，答案与得分在 mark_answer 内。
    '.questionLi',
    '.singleQuesId',
    // OCS 风格：各平台明确声明“每一道题”的 root，而不是扫描整个题型大节。
    '.q_main',
    '.question-item',
    '.questionContent',
    '.question-area-content',
    '.exam-item',
    '.examPaper_subject',
    '.subjectDet',
    '.u-questionItem',
    '[class*=questionBody]',
    '[id*=sigleQuestionDiv]',
    '.questionLi',
    'div:has(> .questionContent)',
    // 常见结果页/考试系统命名。
    '[class*="question-item" i]',
    '[class*="question_box" i]',
    '[class*="question-box" i]',
    '[class*="questionCard" i]',
    '[class*="question-card" i]',
    '[class*="questionItem" i]',
    '[class*="question-item" i]',
    '[class*="subject-item" i]',
    '[class*="topic-item" i]',
    '[class*="exam-subject" i]'
  ];

  const QUESTION_TITLE_SELECTORS = [
    // 优先读取平台已经结构化好的题干/标题节点，避免整块 innerText 把按钮、答案状态、隐藏文本混到题干里。
    '[title]',
    '[aria-label]',
    '.question-title',
    '.questionTitle',
    '.question-stem',
    '.questionStem',
    '.question-name',
    '.questionName',
    '.subject-title',
    '.subjectTitle',
    '.topic-title',
    '.topicTitle',
    '.q-title',
    '.qTitle',
    '.q_content',
    '.qContent',
    '[class*="question-title" i]',
    '[class*="questiontitle" i]',
    '[class*="question-stem" i]',
    '[class*="questionstem" i]',
    '[class*="subject-title" i]',
    '[class*="subjecttitle" i]',
    '[class*="topic-title" i]',
    '[class*="topictitle" i]',
    '[class*="stem" i]'
  ];

  const BANK_TITLE_SELECTORS = [
    // 作业/章节/试卷卡片上的标题区域，例如截图里的“第一章 数据统计导论”。
    '.title',
    '.name',
    '.chapter-title',
    '.chapterTitle',
    '.homework-title',
    '.homeworkTitle',
    '.task-title',
    '.taskTitle',
    '.exam-title',
    '.examTitle',
    '.paper-title',
    '.paperTitle',
    '[class*="chapter" i][class*="title" i]',
    '[class*="homework" i][class*="title" i]',
    '[class*="task" i][class*="title" i]',
    '[class*="exam" i][class*="title" i]',
    '[class*="paper" i][class*="title" i]',
    'h1',
    'h2',
    'h3'
  ];

  const style = document.createElement('style');
  style.textContent = `
    #${PANEL_ID} {
      position: fixed;
      right: 18px;
      bottom: 18px;
      z-index: 2147483647;
      width: 320px;
      padding: 14px;
      color: #172033;
      background: rgba(255, 255, 255, 0.96);
      border: 1px solid rgba(90, 129, 255, 0.28);
      border-radius: 14px;
      box-shadow: 0 12px 36px rgba(31, 45, 75, 0.18);
      font: 14px/1.45 -apple-system, BlinkMacSystemFont, "Segoe UI", "Microsoft YaHei", sans-serif;
    }
    #${PANEL_ID} * { box-sizing: border-box; }
    #${PANEL_ID} .sf-title { display: flex; align-items: center; justify-content: space-between; font-weight: 700; margin-bottom: 8px; }
    #${PANEL_ID} .sf-subtitle { color: #667085; font-size: 12px; margin-bottom: 10px; }
    #${PANEL_ID} .sf-actions { display: grid; grid-template-columns: 1fr 1fr; gap: 8px; margin-bottom: 10px; }
    #${PANEL_ID} button {
      cursor: pointer;
      border: 1px solid rgba(90, 129, 255, 0.35);
      border-radius: 10px;
      padding: 8px 10px;
      color: #3152d4;
      background: #f6f8ff;
      font-weight: 600;
    }
    #${PANEL_ID} button:hover { background: #eef3ff; }
    #${PANEL_ID} .sf-primary { color: white; background: #4568f0; border-color: #4568f0; }
    #${PANEL_ID} .sf-primary:hover { background: #3658d8; }
    #${PANEL_ID} .sf-log {
      max-height: 120px;
      overflow: auto;
      white-space: pre-wrap;
      color: #475467;
      background: #f8fafc;
      border-radius: 10px;
      padding: 8px;
      font-size: 12px;
    }
    #${PANEL_ID} .sf-mini { color: #98a2b3; font-size: 11px; margin-top: 8px; }
    .${SELECTING_CLASS} * { cursor: crosshair !important; }
    .shuaforge-exporter-hover { outline: 3px solid #4568f0 !important; outline-offset: 3px !important; }
  `;
  document.documentElement.appendChild(style);

  function mountPanel() {
    if (document.getElementById(PANEL_ID)) return;

    const panel = document.createElement('div');
    panel.id = PANEL_ID;
    panel.innerHTML = `
      <div class="sf-title">
        <span>ShuaForge 题库导出</span>
        <button type="button" data-action="hide" title="隐藏面板">×</button>
      </div>
      <div class="sf-subtitle">用于已完成答题结果页：提取题目、正确答案、解析；有图片时自动导出 ZIP。</div>
      <div class="sf-actions">
        <button type="button" data-action="select">选择区域</button>
        <button type="button" data-action="scan">扫描预览</button>
        <button type="button" data-action="download" class="sf-primary">导出题库</button>
        <button type="button" data-action="reset">重置区域</button>
      </div>
      <div class="sf-log" data-role="log">默认扫描整个页面。建议在成绩/解析页点击“选择区域”，框住题目列表外层后再导出。</div>
      <div class="sf-mini">只读脚本：不会提交答案、不会修改页面数据。</div>
    `;

    panel.addEventListener('click', (event) => {
      const target = event.target instanceof Element ? event.target : null;
      const button = target?.closest<HTMLButtonElement>('button[data-action]');
      if (!button) return;
      const action = button.dataset.action;
      if (action === 'hide') panel.remove();
      if (action === 'select') startSelecting();
      if (action === 'scan') scanAndPreview();
      if (action === 'download') void downloadCsv();
      if (action === 'reset') {
        selectedRoot = document.body;
        log('已重置为扫描整个页面。');
      }
    });

    document.body.appendChild(panel);
  }

  function log(message: string): void {
    const logEl = document.querySelector(`#${PANEL_ID} [data-role="log"]`);
    if (logEl) logEl.textContent = message;
  }

  function startSelecting() {
    selecting = true;
    document.documentElement.classList.add(SELECTING_CLASS);
    log('选择模式：点击题目列表所在的大容器。按 Esc 取消。');
  }

  function stopSelecting() {
    selecting = false;
    document.documentElement.classList.remove(SELECTING_CLASS);
    document.querySelectorAll('.shuaforge-exporter-hover').forEach((el) => {
      el.classList.remove('shuaforge-exporter-hover');
    });
  }

  document.addEventListener('mouseover', (event) => {
    if (!selecting) return;
    document.querySelectorAll('.shuaforge-exporter-hover').forEach((el) => {
      el.classList.remove('shuaforge-exporter-hover');
    });
    const eventTarget = event.target instanceof Element ? event.target : null;
    const target = eventTarget?.closest<HTMLElement>('section, article, main, div, ul, ol, table, form') || eventTarget;
    if (target && target !== document.documentElement && target !== document.body) {
      target.classList.add('shuaforge-exporter-hover');
    }
  }, true);

  document.addEventListener('click', (event) => {
    if (!selecting) return;
    const panel = document.getElementById(PANEL_ID);
    const eventTarget = event.target instanceof Element ? event.target : null;
    if (!eventTarget) return;
    if (panel && panel.contains(eventTarget)) return;

    event.preventDefault();
    event.stopPropagation();
    selectedRoot = eventTarget.closest<HTMLElement>('section, article, main, div, ul, ol, table, form') || document.body;
    stopSelecting();
    log(`已选择区域：${describeElement(selectedRoot)}\n现在可以点击“扫描预览”或“导出题库”。`);
  }, true);

  document.addEventListener('keydown', (event) => {
    if (event.key === 'Escape' && selecting) {
      stopSelecting();
      log('已取消选择区域。');
    }
  }, true);

  function scanAndPreview(): void {
    lastProblems = extractProblems(selectedRoot);
    if (lastProblems.length === 0) {
      log('没有识别到题目。请进入“已完成/答题结果/答案解析”页面，或点击“选择区域”框住题目列表后重试。');
      return;
    }

    const preview = lastProblems.slice(0, 3).map((item, index) => {
      return `${index + 1}. ${truncate(item.prompt, 42)}\n   答案：${item.answer || '未识别'}\n   解析：${truncate(item.explanation || '无', 36)}`;
    }).join('\n');

    const bankInfo = extractBankInfo(selectedRoot);
    log(`题库：${bankInfo.name}\n识别到 ${lastProblems.length} 道题，图片 ${countImages(lastProblems)} 张。预览：\n${preview}${lastProblems.length > 3 ? '\n...' : ''}`);
  }

  async function downloadCsv(): Promise<void> {
    const problems = lastProblems.length > 0 ? lastProblems : extractProblems(selectedRoot);
    const bankInfo = extractBankInfo(selectedRoot);
    lastProblems = problems;

    if (problems.length === 0) {
      log('没有可导出的题目。先点击“扫描预览”确认页面能识别。');
      return;
    }

    if (countImages(problems) > 0) {
      await downloadZip(problems, bankInfo);
      return;
    }

    const csv = toCsv(problems, bankInfo);
    const blob = new Blob([`\ufeff${csv}`], { type: 'text/csv;charset=utf-8' });
    triggerDownload(blob, `${safeFileName(bankInfo.name)}-${timestamp()}.csv`);
    log(`已导出题库「${bankInfo.name}」共 ${problems.length} 道题。可在 ShuaForge 中导入该 CSV。`);
  }

  async function downloadZip(problems: Problem[], bankInfo: BankInfo): Promise<void> {
    log(`检测到 ${countImages(problems)} 张图片，正在抓取并打包 ZIP...`);
    const ZipCtor = await ensureZipLibrary();
    const zip = new ZipCtor();
    const packagedProblems = await hydrateProblemImages(problems);
    zip.file('problems.json', JSON.stringify({
      deck_name: bankInfo.name,
      deck_info: bankInfo.info,
      exported_at: new Date().toISOString(),
      problem_count: packagedProblems.length,
      problems: packagedProblems
    }, null, 2));
    zip.file('problems.csv', `\ufeff${toCsv(packagedProblems, bankInfo)}`);
    for (const problem of packagedProblems) {
      for (let index = 0; index < problem.images.length; index += 1) {
        const image = problem.images[index];
        if (!image.base64) continue;
        zip.file(`assets/${safeFileName(problem.id)}-${index + 1}-${safeFileName(image.filename)}`, base64ToBytes(image.base64));
      }
    }
    const blob = await zip.generateAsync({ type: 'blob' });
    triggerDownload(blob, `${safeFileName(bankInfo.name)}-${timestamp()}.zip`);
    log(`已导出 ZIP：题目 ${packagedProblems.length} 道，图片 ${countImages(packagedProblems)} 张。`);
  }

  function extractBankInfo(root: HTMLElement): BankInfo {
    const scope = root || document.body;
    const text = textSnapshot(scope).text || textSnapshot(document.body).text;
    const name = extractBankName(scope, text) || normalizeText(document.title).replace(/[-_].*$/, '').trim() || 'ShuaForge题库';
    const count = matchFirst(text, /题量\s*[:：]?\s*(\d+)/) || matchFirst(text, /共\s*(\d+)\s*题/);
    const score = matchFirst(text, /满分\s*[:：]?\s*(\d+)/);
    const userScore = matchFirst(text, /智能分析\s*\n?\s*(\d+(?:\.\d+)?)\s*分/)
      || matchFirst(text, /(^|\n)\s*(\d+(?:\.\d+)?)\s*分\s*(?:\n|$)/, 2);
    const time = matchFirst(text, /作答时间\s*[:：]?\s*([^\n]+)/);
    const info: string[] = [];
    if (count) info.push(`题量:${count}`);
    if (score) info.push(`满分:${score}`);
    if (userScore) info.push(`得分:${userScore}`);
    if (time) info.push(`作答时间:${time}`);
    return {
      name: cleanBankName(name),
      info: info.join('；')
    };
  }

  function extractBankName(scope: HTMLElement, fullText: string): string {
    const candidates: string[] = [];
    for (const selector of BANK_TITLE_SELECTORS) {
      try {
        for (const el of Array.from(scope.querySelectorAll<HTMLElement>(selector))) {
          if (!isVisible(el)) continue;
          const value = cleanBankName(el.getAttribute('title') || el.innerText || el.textContent || '');
          if (looksLikeBankName(value)) candidates.push(value);
        }
      } catch {
        // 忽略不兼容选择器。
      }
    }

    const firstLine = fullText.split('\n').map(cleanBankName).find(looksLikeBankName);
    if (firstLine) candidates.push(firstLine);

    candidates.sort((a, b) => scoreBankName(b) - scoreBankName(a));
    return candidates[0] || '';
  }

  function cleanBankName(value: unknown): string {
    return normalizeText(value)
      .replace(/\s*(?:题量|满分|作答时间|智能分析|得分|分)\s*[:：]?[\s\S]*$/g, '')
      .trim();
  }

  function looksLikeBankName(value: string): boolean {
    if (!value || value.length < 3 || value.length > 80) return false;
    if (/^(题量|满分|作答时间|智能分析|得分|正确答案|答案解析|解析|知识点|我的答案|你的答案)/.test(value)) return false;
    if (/^[A-H][\.、．]/.test(value)) return false;
    return /[\u4e00-\u9fa5A-Za-z0-9]/.test(value);
  }

  function scoreBankName(value: string): number {
    let score = 0;
    if (/第.+章|章节|作业|练习|测试|试卷|考试|导论|统计|数据/.test(value)) score += 4;
    if (value.length >= 4 && value.length <= 32) score += 2;
    if (/ShuaForge|题库导出|浏览器|课程|平台/.test(value)) score -= 2;
    return score;
  }

  function matchFirst(text: string, pattern: RegExp, groupIndex = 1): string {
    const match = text.match(pattern);
    return match && match[groupIndex] ? normalizeText(match[groupIndex]) : '';
  }

  function extractProblems(root: HTMLElement): Problem[] {
    const scope = root || document.body;
    const blocks = findQuestionBlocks(scope);
    const indexedScores = extractIndexedAnswerScores(scope);
    const drafts: ParsedProblemDraft[] = [];
    const seen = new Set<string>();

    for (const block of blocks) {
      const draft = parseProblemBlock(block, drafts.length + 1, indexedScores.get(drafts.length + 1) || null);
      if (!draft || !draft.problem.prompt || !draft.problem.answer) continue;

      const key = `${draft.problem.prompt}::${draft.problem.answer}`;
      if (seen.has(key)) continue;
      seen.add(key);
      drafts.push(draft);
    }

    applySingleChoiceScoreFallback(drafts, extractBankInfo(scope));
    return drafts.map((draft) => draft.problem);
  }

  function extractIndexedAnswerScores(root: HTMLElement): Map<number, AnswerScore> {
    const scores = new Map<number, AnswerScore>();
    const textBlocks = splitTextBlocksByQuestionNumber(root);
    for (const block of textBlocks) {
      const index = Number(block.dataset.virtualIndex || 0);
      if (!index) continue;
      const score = extractAnswerScore(getCleanLines(block));
      if (score) scores.set(index, score);
    }
    return scores;
  }

  function findQuestionBlocks(root: HTMLElement): QuestionBlock[] {
    const selectorBlocks = findQuestionBlocksBySelectors(root);
    if (selectorBlocks.length >= 3) return selectorBlocks;

    const textBlocks = splitTextBlocksByQuestionNumber(root);
    if (textBlocks.length > selectorBlocks.length) return textBlocks;

    return selectorBlocks.sort((a, b) => {
      const pos = a.compareDocumentPosition(b);
      return pos & Node.DOCUMENT_POSITION_FOLLOWING ? -1 : 1;
    });
  }

  function findQuestionBlocksBySelectors(root: HTMLElement): HTMLElement[] {
    const candidates: HTMLElement[] = [];
    for (const selector of QUESTION_ROOT_SELECTORS) {
      try {
        candidates.push(...Array.from(root.querySelectorAll<HTMLElement>(selector)));
      } catch {
        // 某些旧浏览器/脚本管理器不支持 :has，忽略即可。
      }
    }

    const blocks = candidates
      .filter((el) => isVisible(el))
      .filter((el) => looksLikeSingleSolvedQuestion(el))
      .sort((a, b) => getQuestionNumber(a) - getQuestionNumber(b));

    return uniqueBlocks(blocks);
  }

  function uniqueBlocks(blocks: HTMLElement[]): HTMLElement[] {
    const result: HTMLElement[] = [];
    for (const block of blocks) {
      const text = textSnapshot(block).text;
      if (!text) continue;
      if (result.some((existing) => existing === block || existing.contains(block))) continue;

      // 如果当前元素是更细粒度题块，则替换掉之前误收的大父容器。
      for (let index = result.length - 1; index >= 0; index -= 1) {
        if (block.contains(result[index])) result.splice(index, 1);
      }
      result.push(block);
    }
    return result;
  }

  function splitTextBlocksByQuestionNumber(root: HTMLElement): VirtualQuestionBlock[] {
    const text = textSnapshot(root).text;
    if (!text) return [];

    const lines = text.split('\n').map((line) => normalizeText(line)).filter(Boolean);
    const starts: number[] = [];

    for (let index = 0; index < lines.length; index += 1) {
      if (isQuestionStartLine(lines[index])) starts.push(index);
    }

    if (starts.length < 3) return [];

    const blocks: VirtualQuestionBlock[] = [];
    for (let index = 0; index < starts.length; index += 1) {
      const start = starts[index];
      const end = starts[index + 1] ?? lines.length;
      const chunkLines = lines.slice(start, end);
      if (!chunkLines.some((line) => /正确答案|参考答案|标准答案|我的答案|你的答案|作答答案/.test(line))) continue;
      blocks.push(createVirtualBlock(chunkLines.join('\n'), index + 1));
    }

    return blocks;
  }

  function createVirtualBlock(text: string, index: number): VirtualQuestionBlock {
    return {
      nodeType: Node.ELEMENT_NODE,
      tagName: 'VIRTUAL',
      className: VIRTUAL_BLOCK_CLASS,
      innerText: text,
      textContent: text,
      dataset: { virtualIndex: String(index) },
      querySelectorAll() {
        return [];
      },
      compareDocumentPosition(other: QuestionBlock) {
        const otherIndex = Number(other?.dataset?.virtualIndex || 0);
        return Number(this.dataset.virtualIndex) < otherIndex
          ? Node.DOCUMENT_POSITION_FOLLOWING
          : Node.DOCUMENT_POSITION_PRECEDING;
      },
      contains(other: QuestionBlock) {
        return this === other;
      }
    };
  }

  function applySingleChoiceScoreFallback(drafts: ParsedProblemDraft[], bankInfo: BankInfo): void {
    const missing = drafts.filter((draft) => !draft.answerScore && isTemporaryUserAnswer(draft.fullText, draft.answer));
    if (missing.length === 0) return;
    if (!drafts.every((draft) => inferTemporaryAnswerKind(draft.answer, draft.promptLine, draft.options) === 'single_choice')) return;

    const totalScore = extractDeckInfoNumber(bankInfo.info, '得分');
    const fullScore = extractDeckInfoNumber(bankInfo.info, '满分');
    if (!totalScore || !fullScore) return;

    const knownScore = drafts.reduce((total, draft) => total + (draft.answerScore?.value || 0), 0);
    const knownWrongCount = drafts.filter((draft) => draft.answerScore && draft.answerScore.value <= 0).length;
    const scorePerQuestion = fullScore / drafts.length;
    if (!Number.isFinite(scorePerQuestion) || scorePerQuestion <= 0) return;

    const inferredCorrectCount = Math.round((totalScore - knownScore) / scorePerQuestion);
    const exactEvenScoreInference = inferredCorrectCount === missing.length;
    const resultPageHidesCorrectScores = knownWrongCount > 0
      && totalScore > knownScore
      && totalScore < fullScore
      && missing.length + knownWrongCount === drafts.length;
    if (!exactEvenScoreInference && !resultPageHidesCorrectScores) return;

    const inferredScore = formatScore(scorePerQuestion);
    for (const draft of missing) {
      const answerScore: AnswerScore = { value: scorePerQuestion, display: inferredScore };
      draft.answerScore = answerScore;
      draft.problem.explanation = exactEvenScoreInference
        ? `${SCORE_INFERRED_NOTE} 当前页面未把本题得分放在题目块内，但整套题为单选题；按整卷得分 ${formatScore(totalScore)} / ${formatScore(fullScore)} 和已识别错题反推，该作答可视为正确。`
        : `${SCORE_INFERRED_NOTE} 当前页面只在部分题目块显示 0 分错题，未显示本题题内得分；结合整卷得分 ${formatScore(totalScore)} / ${formatScore(fullScore)}、已识别 ${knownWrongCount} 道 0 分题，以及本套题均为单选题，可将该作答暂按正确处理。`;
      draft.problem.tags = draft.problem.tags.filter((tag) => tag !== '未批改' && tag !== 'AI批改');
      addScoreInferenceTags(draft.problem.tags, draft.fullText, draft.answer, answerScore, draft.promptLine, draft.options);
      for (const tag of [exactEvenScoreInference ? '整卷得分反推' : '结果页显示规则反推', '作答正确']) {
        if (!draft.problem.tags.includes(tag)) draft.problem.tags.push(tag);
      }
    }
  }

  function extractDeckInfoNumber(info: string, label: string): number {
    const match = info.match(new RegExp(`${label}:([0-9]+(?:\\.[0-9]+)?)`));
    if (!match?.[1]) return 0;
    const value = Number(match[1]);
    return Number.isFinite(value) ? value : 0;
  }

  function formatScore(value: number): string {
    return value.toFixed(4).replace(/0+$/g, '').replace(/\.$/g, '');
  }

  function chooseBestBlock(el: HTMLElement, root: HTMLElement): HTMLElement {
    let current = el;
    let best = el;
    let bestScore = scoreBlock(el);

    while (current.parentElement && current.parentElement !== root && current.parentElement !== document.body) {
      current = current.parentElement;
      const score = scoreBlock(current);
      const textLength = textSnapshot(current).text.length;
      if (score >= bestScore && textLength < 5000) {
        best = current;
        bestScore = score;
      }
      if (score >= 8 && textLength > 160) break;
    }

    return best;
  }

  function looksLikeSingleSolvedQuestion(el: HTMLElement): boolean {
    const text = textSnapshot(el).text;
    if (text.length < 20 || text.length > 3500) return false;

    const questionStartCount = countQuestionStarts(text);
    if (questionStartCount > 1) return false;

    const hasAnswer = /正确答案|参考答案|标准答案|我的答案|你的答案|作答答案/.test(text);
    const hasResultInfo = /答案解析|解析|我的答案|你的答案|作答答案|得分/.test(text);
    const hasOptions = /(?:^|\n)\s*-?\s*[A-H][\.、．]/.test(text);
    return hasAnswer && (hasResultInfo || hasOptions);
  }

  function scoreBlock(el: HTMLElement): number {
    const text = textSnapshot(el).text;
    let score = 0;
    if (/正确答案|参考答案|标准答案/.test(text)) score += 3;
    if (/我的答案|你的答案|作答答案/.test(text)) score += 2;
    if (/答案解析|解析/.test(text)) score += 2;
    if (/知识点|考点|标签/.test(text)) score += 1;
    if (/\b[A-H][\.、．]/.test(text)) score += 2;
    if (/^\s*\d+[\.、]/m.test(text)) score += 1;
    if (text.length > 80 && text.length < 2500) score += 1;
    return score;
  }

  function parseProblemBlock(block: QuestionBlock, index: number, fallbackScore: AnswerScore | null = null): ParsedProblemDraft | null {
    const rawLines = getCleanLines(block);
    if (rawLines.length === 0) return null;

    const fullText = rawLines.join('\n');
    const answer = extractStructuredUserAnswer(block) || extractAnswer(fullText);
    if (!answer) return null;

    const promptLine = extractPromptLine(rawLines, block);
    const options = extractOptions(rawLines);
    const answerScore = extractStructuredAnswerScore(block) || extractAnswerScore(rawLines) || fallbackScore;
    const explanation = extractExplanation(rawLines, fullText, answer, answerScore, promptLine, options);
    const tags = extractTags(rawLines, block);
    if (isTemporaryUserAnswer(fullText, answer) && !answerScore) {
      for (const tag of ['未批改', 'AI批改']) {
        if (!tags.includes(tag)) tags.push(tag);
      }
    }
    addScoreInferenceTags(tags, fullText, answer, answerScore, promptLine, options);
    const imageCandidates = textSnapshot(block).images;
    const imageLines = imageCandidates.map((image, imageIndex) => `${IMAGE_PLACEHOLDER_PREFIX}${imageIndex + 1}: ${image.src}`);
    const prompt = [promptLine, ...imageLines, ...options].filter(Boolean).join('\n');

    if (!prompt || prompt.length < 4) return null;

    const problem = {
      id: buildId(index, prompt),
      prompt,
      answer,
      explanation,
      tags,
      images: imageCandidates.map((image, imageIndex) => ({
        filename: guessImageFilename(image.src, index, imageIndex),
        mime_type: guessMimeType(image.src),
        base64: image.src.startsWith('data:') ? dataUriToBase64(image.src) : '',
        alt_text: image.alt,
        source_url: image.src
      }))
    };

    return { problem, promptLine, options, answer, fullText, answerScore };
  }

  function getCleanLines(block: QuestionBlock): string[] {
    return textSnapshot(block).text
      .replace(/\u00a0/g, ' ')
      .split(/\n+/)
      .map((line: string) => normalizeText(line))
      .filter(Boolean)
      .filter((line: string) => !/^AI\s*讲解$/i.test(line))
      .filter((line: string) => !/^智能分析$/.test(line));
  }

  function isQuestionStartLine(line: string): boolean {
    return /^(?:#{1,6}\s*)?\d+\s*(?:、|[\.．](?!\d))\s*(?:\([^)]*题\)|（[^）]*题）|\[[^\]]*题\]|【[^】]*题】)?\s*\S+/.test(line)
      && !/^(\d+\s*(?:、|[\.．](?!\d))\s*)?(我的答案|你的答案|正确答案|参考答案|标准答案|答案解析|解析|知识点|得分)/.test(line);
  }

  function countQuestionStarts(text: string): number {
    return text.split('\n').filter((line: string) => isQuestionStartLine(normalizeText(line))).length;
  }

  function getQuestionNumber(el: HTMLElement): number {
    const text = textSnapshot(el).text;
    const match = text.match(/^(?:#{1,6}\s*)?(\d+)\s*(?:、|[\.．](?!\d))/m);
    return match ? Number(match[1]) : Number.MAX_SAFE_INTEGER;
  }

  function extractPromptLine(lines: string[], block: QuestionBlock): string {
    const questionIndex = lines.findIndex((line: string) => /^(?:#{1,6}\s*)?\d+\s*(?:、|[\.．](?!\d))/.test(line));
    if (questionIndex >= 0) {
      const promptLines: string[] = [];
      for (let index = questionIndex; index < lines.length; index += 1) {
        const line = lines[index];
        if (index > questionIndex && isPromptBoundaryLine(line)) break;
        const value = index === questionIndex
          ? line
            .replace(/^#{1,6}\s*/, '')
            .replace(/^\d+\s*(?:、|[\.．](?!\d))\s*/, '')
            .replace(/^\([^)]*题\)\s*/, '')
            .replace(/^（[^）]*题）\s*/, '')
            .trim()
          : line;
        const cleaned = cleanPromptText(value);
        if (cleaned) promptLines.push(cleaned);
      }
      const prompt = promptLines.join('\n').trim();
      if (prompt) return prompt;
    }

    const structuredTitle = extractStructuredTitle(block);
    if (structuredTitle) return structuredTitle;

    const fallback = lines.find((line: string) => {
      if (/^(我的答案|你的答案|正确答案|参考答案|标准答案|答案解析|解析|知识点|得分)[:：]?/.test(line)) return false;
      if (/^[A-H][\.、．]/.test(line)) return false;
      return line.length >= 6;
    }) || '';

    return cleanPromptText(fallback);
  }

  function isPromptBoundaryLine(line: string): boolean {
    if (/^-?\s*[A-H][\.、．]/.test(line)) return true;
    return /^\*{0,2}(我的答案|你的答案|作答答案|正确答案|参考答案|标准答案|答案解析|解析|知识点|考点|标签|得分)\*{0,2}\s*[:：]?/.test(line);
  }

  function extractStructuredTitle(block: QuestionBlock): string {
    if (!block || typeof block.querySelectorAll !== 'function') return '';

    const candidates = [];
    for (const selector of QUESTION_TITLE_SELECTORS) {
      try {
        for (const el of Array.from(block.querySelectorAll(selector) as ArrayLike<HTMLElement>)) {
          if (!isVisible(el)) continue;
          if (isNonPromptElement(el)) continue;
          const title = normalizeText(el.getAttribute('title') || '');
          const ariaLabel = normalizeText(el.getAttribute('aria-label') || '');
          const text = textSnapshot(el).text;
          for (const value of [title, ariaLabel, text]) {
            const cleaned = cleanPromptText(value);
            if (looksLikePromptTitle(cleaned)) candidates.push(cleaned);
          }
        }
      } catch {
        // 忽略不兼容选择器。
      }
    }

    candidates.sort((a, b) => scorePromptTitle(b) - scorePromptTitle(a));
    return candidates[0] || '';
  }

  function cleanPromptText(value: unknown): string {
    return normalizeText(value)
      .replace(/^#{1,6}\s*/, '')
      .replace(/^\d+\s*(?:、|[\.．](?!\d))\s*/, '')
      .replace(/^\([^)]*题\)\s*/, '')
      .replace(/^（[^）]*题）\s*/, '')
      .replace(/^题目\s*[:：]?\s*/, '')
      .replace(/\s*(?:我的答案|你的答案|正确答案|参考答案|标准答案|答案解析|解析|知识点|得分)\s*[:：]?[\s\S]*$/g, '')
      .trim();
  }

  function looksLikePromptTitle(value: string): boolean {
    if (!value || value.length < 4 || value.length > 500) return false;
    if (/^(我的答案|你的答案|正确答案|参考答案|标准答案|答案解析|解析|知识点|得分|收藏|纠错|AI\s*讲解)[:：]?/.test(value)) return false;
    if (/^-?\s*[A-H][\.、．]/.test(value)) return false;
    if (!/[\u4e00-\u9fa5A-Za-z0-9]/.test(value)) return false;
    return true;
  }

  function isNonPromptElement(el: HTMLElement): boolean {
    if (el.closest('button, input, select, textarea, label')) return true;
    const text = textSnapshot(el).text || normalizeText(el.getAttribute('title') || '');
    if (/^-?\s*[A-H][\.、．]/.test(text)) return true;
    if (/^(我的答案|你的答案|正确答案|参考答案|标准答案|答案解析|解析|知识点|得分|收藏|纠错|AI\s*讲解)[:：]?/.test(text)) return true;

    const classAndId = `${el.className || ''} ${el.id || ''}`.toLowerCase();
    return /answer|option|choice|analysis|explain|result|score|tag|knowledge|button|btn/.test(classAndId);
  }

  function scorePromptTitle(value: string): number {
    let score = 0;
    if (/[？?]$/.test(value)) score += 3;
    if (/\(|（|根据|下列|关于|以下|哪|什么|如何|为什么|判断|计算|选择/.test(value)) score += 2;
    if (value.length >= 8 && value.length <= 220) score += 2;
    if (/正确答案|参考答案|标准答案|答案解析|我的答案|你的答案/.test(value)) score -= 8;
    if (/^-?\s*[A-H][\.、．]/.test(value)) score -= 6;
    return score;
  }

  function extractOptions(lines: string[]): string[] {
    const options: string[] = [];
    for (const line of lines) {
      if (/^-?\s*[A-H][\.、．]\s*/.test(line)) {
        options.push(line.replace(/^-?\s*([A-H])[\.、．]\s*/, '$1. '));
      }
    }
    return options;
  }

  function extractAnswer(text: string): string {
    const patterns = [
      /正确答案\s*[:：]?\s*([A-H](?:\s*[,，、]\s*[A-H])*)/i,
      /参考答案\s*[:：]?\s*([A-H](?:\s*[,，、]\s*[A-H])*)/i,
      /标准答案\s*[:：]?\s*([A-H](?:\s*[,，、]\s*[A-H])*)/i,
      /答案\s*[:：]?\s*([A-H](?:\s*[,，、]\s*[A-H])*)/i,
      /正确答案\s*[:：]?\s*([^\n]+)/,
      /参考答案\s*[:：]?\s*([^\n]+)/,
      /标准答案\s*[:：]?\s*([^\n]+)/,
      /\*{0,2}我的答案\*{0,2}\s*[:：]?\s*([^\n]+)/,
      /\*{0,2}你的答案\*{0,2}\s*[:：]?\s*([^\n]+)/,
      /\*{0,2}作答答案\*{0,2}\s*[:：]?\s*([^\n]+)/
    ];

    for (const pattern of patterns) {
      const match = text.match(pattern);
      if (match && match[1]) {
        const answer = cleanExtractedAnswer(match[1]);
        if (answer) return answer;
      }
    }

    return '';
  }

  function cleanExtractedAnswer(value: string): string {
    const normalized = normalizeAnswer(value)
      .replace(/^\*+|\*+$/g, '')
      .replace(/(?:扫描预览|选择区域|重置区域|导出 CSV|ShuaForge 题库导出).*/g, '')
      .replace(/(?:答案解析|解析)\s*$/g, '')
      .replace(/^[,，、;；\s]+|[,，、;；\s]+$/g, '')
      .trim();

    const labelledChoices = extractLabelledChoiceAnswer(normalized);
    if (labelledChoices) return labelledChoices;

    const choiceWithText = normalized.match(/^([A-H])\s*[:：]\s*.+$/i);
    const answer = choiceWithText ? choiceWithText[1].toUpperCase() : normalized;

    if (!answer) return '';
    if (/^(解析|导出 CSV|扫描预览|选择区域|重置区域)$/i.test(answer)) return '';
    return answer;
  }

  function extractLabelledChoiceAnswer(value: string): string {
    const matches = Array.from(value.matchAll(/(?:^|[,，、;；\s])([A-H])\s*[:：]/gi));
    if (matches.length === 0) return '';
    const choices = Array.from(new Set(matches.map((match) => match[1].toUpperCase())));
    return choices.join(',');
  }

  function extractStructuredUserAnswer(block: QuestionBlock): string {
    if (!isRealElement(block)) return '';
    const candidates = [
      '.stuAnswerContent',
      '[class*="stuAnswer" i]',
      '[class*="myAnswer" i]',
      '[class*="userAnswer" i]'
    ];

    for (const selector of candidates) {
      try {
        const el = block.querySelector<HTMLElement>(selector);
        const text = el ? normalizeText(el.innerText || el.textContent || '') : '';
        const answer = cleanExtractedAnswer(text);
        if (answer) return answer;
      } catch {
        // 忽略不兼容选择器。
      }
    }
    return '';
  }

  function extractStructuredAnswerScore(block: QuestionBlock): AnswerScore | null {
    if (!isRealElement(block)) return null;
    const candidates = [
      '.mark_score .totalScore i.custom-style',
      '.mark_score .totalScore i',
      '.mark_score .totalScore',
      '.mark_score [class*="totalScore" i] i',
      '.mark_score [class*="totalScore" i]',
      '.totalScore',
      '[class*="score" i]'
    ];

    for (const selector of candidates) {
      try {
        for (const el of Array.from(block.querySelectorAll<HTMLElement>(selector))) {
          const text = normalizeText(el.innerText || el.textContent || '');
          const score = parseScoreText(text) || parseBareScoreText(text);
          if (score) return score;
        }
      } catch {
        // 忽略不兼容选择器。
      }
    }
    return null;
  }

  function parseBareScoreText(text: string): AnswerScore | null {
    const match = normalizeText(text).match(/^\d+(?:\.\d+)?$/);
    if (!match) return null;
    const value = Number(match[0]);
    if (!Number.isFinite(value)) return null;
    return { value, display: match[0] };
  }

  function extractAnswerScore(lines: string[]): AnswerScore | null {
    for (const line of lines) {
      if (/题量|满分|作答时间|智能分析/.test(line)) continue;
      const score = parseScoreText(line);
      if (score) return score;
    }
    return null;
  }

  function parseScoreText(text: string): AnswerScore | null {
    const match = normalizeText(text).match(/(?:^|[;；\s])\*?\s*(\d+(?:\.\d+)?)\s*\*?\s*分\s*$/);
    if (!match?.[1]) return null;
    const value = Number(match[1]);
    if (!Number.isFinite(value)) return null;
    return { value, display: match[1] };
  }

  function extractExplanation(
    lines: string[],
    fullText: string,
    answer: string,
    answerScore: AnswerScore | null,
    promptLine: string,
    options: string[]
  ): string {
    const inline = fullText.match(/(?:答案解析|解析)\s*[:：]\s*([\s\S]*?)(?:\n\s*(?:知识点|考点|标签|得分)\s*[:：]?|$)/);
    if (inline && inline[1]) return normalizeText(inline[1]);

    const index = lines.findIndex((line: string) => /^(答案解析|解析)[:：]?/.test(line));
    if (index < 0) {
      if (isTemporaryUserAnswer(fullText, answer) && answerScore) {
        return buildScoreInferredExplanation(answerScore, answer, promptLine, options);
      }
      return isTemporaryUserAnswer(fullText, answer) ? PENDING_REVIEW_NOTE : '';
    }

    const chunks: string[] = [];
    for (let i = index; i < lines.length; i += 1) {
      const line = lines[i].replace(/^(答案解析|解析)[:：]?\s*/, '').trim();
      if (i > index && /^(知识点|考点|标签|得分)[:：]?/.test(line)) break;
      if (line) chunks.push(line);
    }
    return chunks.join(' ');
  }

  function buildScoreInferredExplanation(answerScore: AnswerScore, answer: string, promptLine: string, options: string[]): string {
    const kind = inferTemporaryAnswerKind(answer, promptLine, options);
    if (kind === 'single_choice') {
      return answerScore.value > 0
        ? `${SCORE_INFERRED_NOTE} 当前作答得分 ${answerScore.display} 分，单选题可推断该答案正确。`
        : `${SCORE_INFERRED_NOTE} 当前作答得分 0 分，单选题可推断该作答错误；因页面未给出标准答案，需人工补全正确答案。`;
    }
    if (kind === 'multiple_choice') {
      return `${SCORE_INFERRED_NOTE} 当前作答得分 ${answerScore.display} 分；多选题可能存在少选/多选/部分得分规则，需人工复核后再作为标准答案使用。`;
    }
    return `${SCORE_INFERRED_NOTE} 当前作答得分 ${answerScore.display} 分；填空/主观题评分规则不确定，需人工复核后再作为标准答案使用。`;
  }

  function addScoreInferenceTags(
    tags: string[],
    fullText: string,
    answer: string,
    answerScore: AnswerScore | null,
    promptLine: string,
    options: string[]
  ): void {
    if (!answerScore || !isTemporaryUserAnswer(fullText, answer)) return;

    const kind = inferTemporaryAnswerKind(answer, promptLine, options);
    const inferredTags = ['无标准答案', '按得分推断', `本题得分:${answerScore.display}分`];

    if (kind === 'single_choice') {
      inferredTags.push(answerScore.value > 0 ? '作答正确' : '作答错误');
      if (answerScore.value <= 0) inferredTags.push('待复核');
    } else if (kind === 'multiple_choice') {
      inferredTags.push('多选需复核');
      if (answerScore.value <= 0) inferredTags.push('作答错误');
    } else {
      inferredTags.push('填空需复核');
      if (answerScore.value <= 0) inferredTags.push('作答错误');
    }

    for (const tag of inferredTags) {
      if (!tags.includes(tag)) tags.push(tag);
    }
  }

  function inferTemporaryAnswerKind(answer: string, promptLine: string, options: string[]): 'single_choice' | 'multiple_choice' | 'text' {
    if (options.length < 2) return 'text';
    const normalizedChoice = normalizeChoiceLikeAnswer(answer);
    if (!normalizedChoice) return 'text';
    const prompt = normalizeText(promptLine);
    if (normalizedChoice.length > 1 || /多选题|多项选择|不定项|哪些|正确的有|包括|属于.+有|因素有|方法有/.test(prompt)) {
      return 'multiple_choice';
    }
    return 'single_choice';
  }

  function normalizeChoiceLikeAnswer(value: string): string {
    const choices = normalizeAnswer(value)
      .split(/[,，、;；#\s]+/)
      .flatMap((part) => part.split(''))
      .filter((ch) => /^[A-H]$/i.test(ch))
      .map((ch) => ch.toUpperCase());
    return Array.from(new Set(choices)).join('');
  }

  function isTemporaryUserAnswer(text: string, answer: string): boolean {
    return Boolean(answer)
      && !/正确答案|参考答案|标准答案/.test(text)
      && /我的答案|你的答案|作答答案/.test(text);
  }

  function extractTags(lines: string[], block: QuestionBlock): string[] {
    const tags = new Set<string>();
    for (const line of lines) {
      const match = line.match(/^(?:知识点|考点|标签)\s*[:：]?\s*(.+)$/);
      if (!match) continue;
      splitTags(match[1]).forEach((tag: string) => tags.add(tag));
    }

    Array.from(block.querySelectorAll('button, .tag, [class*="tag" i], [class*="knowledge" i], [class*="point" i]') as ArrayLike<HTMLElement>).forEach((el) => {
      const text = textSnapshot(el).text;
      if (text && text.length <= 24 && !/AI|讲解|解析|答案/.test(text)) tags.add(text);
    });

    return Array.from(tags);
  }

  function splitTags(value: string): string[] {
    return value
      .split(/[,，、/|;；\s]+/)
      .map((tag: string) => tag.trim())
      .filter((tag: string) => tag.length > 0 && tag.length <= 24);
  }

  function toCsv(problems: Problem[], bankInfo: BankInfo): string {
    const rows: string[][] = [['id', 'prompt', 'answer', 'explanation', 'tags', 'deck_name', 'deck_info', 'images']];
    for (const problem of problems) {
      rows.push([
        problem.id,
        problem.prompt,
        problem.answer,
        problem.explanation || '',
        (problem.tags || []).join(','),
        bankInfo.name,
        bankInfo.info,
        JSON.stringify(problem.images || [])
      ]);
    }
    return rows.map((row) => row.map(csvCell).join(',')).join('\r\n');
  }

  function csvCell(value: unknown): string {
    const text = String(value ?? '');
    return `"${text.replace(/"/g, '""')}"`;
  }

  function normalizeAnswer(value: unknown): string {
    return normalizeText(value)
      .replace(/[，、]/g, ',')
      .replace(/\s*,\s*/g, ',')
      .replace(/[。；;].*$/, '')
      .trim();
  }

  function normalizeText(value: unknown): string {
    return String(value || '')
      .replace(/\r/g, '\n')
      .replace(/[ \t]+/g, ' ')
      .replace(/\n[ \t]+/g, '\n')
      .trim();
  }

  function textSnapshot(block: QuestionBlock): TextSnapshot {
    if (!isRealElement(block)) {
      return { text: normalizeText(block.innerText || block.textContent || ''), images: [] };
    }

    const clone = block.cloneNode(true) as HTMLElement;
    stripExporterUi(clone);
    const images = collectImageSources(block);
    return {
      text: normalizeText(clone.innerText || clone.textContent || ''),
      images
    };
  }

  function stripExporterUi(root: HTMLElement): void {
    root.querySelector(`#${PANEL_ID}`)?.remove();
    root.querySelectorAll(`#${PANEL_ID}, style, script, noscript, .shuaforge-exporter-hover`).forEach((el) => el.remove());
    root.querySelectorAll('[data-action], [data-role="log"]').forEach((el) => {
      if (el.closest(`#${PANEL_ID}`)) el.remove();
    });
  }

  function collectImageSources(block: HTMLElement): ImageCandidate[] {
    const sources = new Map<string, ImageCandidate>();
    block.querySelectorAll<HTMLImageElement>('img').forEach((img) => {
      const src = img.currentSrc || img.src || img.getAttribute('data-src') || img.getAttribute('data-original') || '';
      const alt = normalizeText(img.alt || img.title || '');
      if (src) sources.set(src, { src, alt });
    });
    block.querySelectorAll<HTMLElement>('[style*="background-image"]').forEach((el) => {
      const style = el.getAttribute('style') || '';
      const match = style.match(/url\(["']?([^"')]+)["']?\)/i);
      if (match?.[1]) sources.set(match[1], { src: match[1], alt: normalizeText(el.getAttribute('aria-label') || '') });
    });
    return Array.from(sources.values());
  }

  function isRealElement(block: QuestionBlock): block is HTMLElement {
    return block instanceof HTMLElement;
  }

  function buildId(index: number, prompt: string): string {
    return `web-${String(index).padStart(4, '0')}-${hashText(prompt).slice(0, 6)}`;
  }

  function hashText(text: string): string {
    let hash = 2166136261;
    for (let i = 0; i < text.length; i += 1) {
      hash ^= text.charCodeAt(i);
      hash = Math.imul(hash, 16777619);
    }
    return (hash >>> 0).toString(16);
  }

  function truncate(value: unknown, size: number): string {
    const text = normalizeText(value);
    return text.length > size ? `${text.slice(0, size)}...` : text;
  }

  function safeFileName(value: unknown): string {
    const name = cleanBankName(value) || 'ShuaForge题库';
    return name.replace(/[\\/:*?"<>|]/g, '_').slice(0, 80);
  }

  function timestamp(): string {
    return new Date().toISOString().slice(0, 19).replace(/[T:]/g, '-');
  }

  function triggerDownload(blob: Blob, filename: string): void {
    const url = URL.createObjectURL(blob);
    const link = document.createElement('a');
    link.href = url;
    link.download = filename;
    document.body.appendChild(link);
    link.click();
    link.remove();
    URL.revokeObjectURL(url);
  }

  function countImages(problems: Problem[]): number {
    return problems.reduce((total, problem) => total + (problem.images?.length || 0), 0);
  }

  async function ensureZipLibrary(): Promise<new () => ZipArchive> {
    if (typeof JSZip === 'function') return JSZip;
    await new Promise<void>((resolve, reject) => {
      const script = document.createElement('script');
      script.src = JSZIP_CDN;
      script.onload = () => resolve();
      script.onerror = () => reject(new Error('JSZip 加载失败，无法导出含图片 ZIP。'));
      document.head.appendChild(script);
    });
    if (typeof JSZip !== 'function') throw new Error('JSZip 未加载。');
    return JSZip;
  }

  async function hydrateProblemImages(problems: Problem[]): Promise<Problem[]> {
    const result: Problem[] = [];
    let failedCount = 0;
    for (const problem of problems) {
      const images: ProblemImage[] = [];
      for (const image of problem.images) {
        if (image.base64) {
          images.push(image);
          continue;
        }
        try {
          const fetched = await fetchImageAsBase64(image.source_url);
          images.push({ ...image, base64: fetched.base64, mime_type: fetched.mime_type || image.mime_type });
        } catch (error) {
          failedCount += 1;
          console.warn('[ShuaForge] 图片抓取失败，已保留 source_url：', image.source_url, error);
          images.push({ ...image, base64: '' });
        }
      }
      result.push({ ...problem, images });
    }
    if (failedCount > 0) log(`有 ${failedCount} 张图片受跨域/鉴权限制未能内嵌，已在题目 images.source_url 中保留原地址。`);
    return result;
  }

  async function fetchImageAsBase64(src: string): Promise<{ base64: string; mime_type: string }> {
    if (src.startsWith('data:')) {
      return { base64: dataUriToBase64(src), mime_type: guessMimeType(src) };
    }
    const absoluteUrl = new URL(src, location.href).toString();
    try {
      return await fetchImageAsBase64WithFetch(absoluteUrl);
    } catch (error) {
      console.warn('[ShuaForge] fetch 图片失败，尝试 GM_xmlhttpRequest：', absoluteUrl, error);
      return fetchImageAsBase64WithGm(absoluteUrl);
    }
  }

  async function fetchImageAsBase64WithFetch(url: string): Promise<{ base64: string; mime_type: string }> {
    const response = await fetch(url, { credentials: 'omit', mode: 'cors', referrer: location.href });
    if (!response.ok) throw new Error(`图片下载失败：${response.status}`);
    const blob = await response.blob();
    const base64 = await blobToBase64(blob);
    return { base64, mime_type: blob.type || guessMimeType(url) };
  }

  function fetchImageAsBase64WithGm(url: string): Promise<{ base64: string; mime_type: string }> {
    if (typeof GM_xmlhttpRequest !== 'function') throw new Error('当前脚本管理器不支持 GM_xmlhttpRequest。');
    return new Promise((resolve, reject) => {
      GM_xmlhttpRequest({
        method: 'GET',
        url,
        responseType: 'blob',
        timeout: 15000,
        headers: { Referer: location.href },
        onload: async (response) => {
          try {
            if (response.status < 200 || response.status >= 300) {
              reject(new Error(`GM 图片下载失败：${response.status}`));
              return;
            }
            const blob = response.response;
            const base64 = await blobToBase64(blob);
            resolve({ base64, mime_type: blob.type || responseHeader(response.responseHeaders || '', 'content-type') || guessMimeType(url) });
          } catch (error) {
            reject(error);
          }
        },
        onerror: (error) => reject(error instanceof Error ? error : new Error('GM 图片下载失败')),
        ontimeout: () => reject(new Error('GM 图片下载超时'))
      });
    });
  }

  function responseHeader(headers: string, name: string): string {
    const target = name.toLowerCase();
    const line = headers.split(/\r?\n/).find((item) => item.toLowerCase().startsWith(`${target}:`));
    return line ? line.slice(line.indexOf(':') + 1).trim().split(';')[0] : '';
  }

  function blobToBase64(blob: Blob): Promise<string> {
    return new Promise((resolve, reject) => {
      const reader = new FileReader();
      reader.onload = () => resolve(dataUriToBase64(String(reader.result || '')));
      reader.onerror = () => reject(reader.error || new Error('图片读取失败'));
      reader.readAsDataURL(blob);
    });
  }

  function base64ToBytes(base64: string): Uint8Array {
    const binary = atob(base64);
    const bytes = new Uint8Array(binary.length);
    for (let index = 0; index < binary.length; index += 1) bytes[index] = binary.charCodeAt(index);
    return bytes;
  }

  function dataUriToBase64(uri: string): string {
    const index = uri.indexOf(',');
    return index >= 0 ? uri.slice(index + 1) : uri;
  }

  function guessMimeType(src: string): string {
    const dataMatch = src.match(/^data:([^;,]+)/i);
    if (dataMatch?.[1]) return dataMatch[1];
    if (/\.jpe?g(?:$|[?#])/i.test(src)) return 'image/jpeg';
    if (/\.webp(?:$|[?#])/i.test(src)) return 'image/webp';
    if (/\.gif(?:$|[?#])/i.test(src)) return 'image/gif';
    if (/\.svg(?:$|[?#])/i.test(src)) return 'image/svg+xml';
    return 'image/png';
  }

  function guessImageFilename(src: string, problemIndex: number, imageIndex: number): string {
    const extension = guessMimeType(src).split('/').pop()?.replace('jpeg', 'jpg').replace('svg+xml', 'svg') || 'png';
    try {
      const url = new URL(src, location.href);
      const name = url.pathname.split('/').pop();
      if (name) return safeFileName(name);
    } catch {
      // data URI or invalid URL.
    }
    return `problem-${problemIndex}-image-${imageIndex + 1}.${extension}`;
  }

  function exposeQuestionBankApi(): void {
    const api = {
      scan(root?: HTMLElement) {
        const scope = root || selectedRoot || document.body;
        lastProblems = extractProblems(scope);
        return { count: lastProblems.length, image_count: countImages(lastProblems), bank: extractBankInfo(scope) };
      },
      listProblems(cursor = 0, limit = 30) {
        if (lastProblems.length === 0) lastProblems = extractProblems(selectedRoot || document.body);
        return {
          cursor,
          limit,
          total: lastProblems.length,
          next_cursor: cursor + limit < lastProblems.length ? cursor + limit : null,
          problems: lastProblems.slice(cursor, cursor + limit)
        };
      },
      getProblem(id: string) {
        if (lastProblems.length === 0) lastProblems = extractProblems(selectedRoot || document.body);
        return lastProblems.find((problem) => problem.id === id) || null;
      }
    };
    Object.defineProperty(window, 'ShuaForgeQuestionBank', { value: api, configurable: true });
  }

  function isVisible(el: HTMLElement): boolean {
    const rect = el.getBoundingClientRect();
    const style = window.getComputedStyle(el);
    return rect.width > 0 && rect.height > 0 && style.display !== 'none' && style.visibility !== 'hidden';
  }

  function describeElement(el: HTMLElement | null): string {
    if (!el) return '未知区域';
    const id = el.id ? `#${el.id}` : '';
    const className = typeof el.className === 'string' && el.className.trim()
      ? `.${el.className.trim().split(/\s+/).slice(0, 3).join('.')}`
      : '';
    return `${el.tagName.toLowerCase()}${id}${className}`;
  }

  if (typeof GM_registerMenuCommand === 'function') {
    GM_registerMenuCommand('显示 ShuaForge 题库导出器', mountPanel);
  }

  exposeQuestionBankApi();
  mountPanel();
})();
