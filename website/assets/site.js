// Zuko website: mobile menu, click-to-play video, copy buttons, docs table of contents.

(function () {
  // Mobile menu.
  const toggle = document.querySelector(".nav-toggle");
  const links = document.querySelector(".nav-links");
  if (toggle && links) {
    toggle.addEventListener("click", () => {
      const open = links.classList.toggle("open");
      toggle.setAttribute("aria-expanded", String(open));
      toggle.textContent = open ? "Close" : "Menu";
    });
  }

  // Video: show the thumbnail first and load YouTube only when someone presses play.
  document.querySelectorAll("[data-youtube]").forEach((box) => {
    const button = box.querySelector("button");
    if (!button) return;
    button.addEventListener("click", () => {
      const id = box.getAttribute("data-youtube");
      const frame = document.createElement("iframe");
      frame.src = `https://www.youtube-nocookie.com/embed/${id}?autoplay=1&rel=0`;
      frame.title = "Zuko demo video";
      frame.allow = "accelerometer; autoplay; clipboard-write; encrypted-media; gyroscope; picture-in-picture";
      frame.allowFullscreen = true;
      box.replaceChildren(frame);
    });
  });

  // Copy buttons on code blocks.
  document.querySelectorAll("pre").forEach((pre) => {
    const code = pre.querySelector("code");
    if (!code) return;
    const btn = document.createElement("button");
    btn.className = "copy";
    btn.type = "button";
    btn.textContent = "Copy";
    btn.addEventListener("click", async () => {
      try {
        await navigator.clipboard.writeText(code.innerText.trim());
        btn.textContent = "Copied";
      } catch {
        btn.textContent = "Press Ctrl+C";
      }
      setTimeout(() => (btn.textContent = "Copy"), 1600);
    });
    pre.appendChild(btn);
  });

  // Docs: highlight the section being read.
  const tocLinks = Array.from(document.querySelectorAll(".toc a[href^='#']"));
  if (tocLinks.length && "IntersectionObserver" in window) {
    const byId = new Map(tocLinks.map((a) => [a.getAttribute("href").slice(1), a]));
    const observer = new IntersectionObserver(
      (entries) => {
        entries.forEach((entry) => {
          if (!entry.isIntersecting) return;
          tocLinks.forEach((a) => a.classList.remove("active"));
          const link = byId.get(entry.target.id);
          if (link) link.classList.add("active");
        });
      },
      { rootMargin: "-90px 0px -70% 0px" }
    );
    byId.forEach((_, id) => {
      const el = document.getElementById(id);
      if (el) observer.observe(el);
    });
  }

  document.querySelectorAll("[data-year]").forEach((el) => (el.textContent = String(new Date().getFullYear())));
})();
