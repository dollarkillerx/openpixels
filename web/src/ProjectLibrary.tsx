import { useState } from "react";
import type { Project } from "./store";

interface Props {
  projects: Project[];
  activeId: number | null;
  photoCount: number;
  onOpen: (project: Project) => void;
  onCreate: (name: string, withCurrentPhotos: boolean) => void;
  onRename: (project: Project, name: string) => void;
  onDelete: (project: Project) => void;
  onLeave: () => void;
  onClose: () => void;
}

export function ProjectLibrary({ projects, activeId, photoCount, onOpen, onCreate, onRename, onDelete, onLeave, onClose }: Props) {
  const [name, setName] = useState("");
  const [withCurrent, setWithCurrent] = useState(true);
  const [renaming, setRenaming] = useState<{ id: number; name: string } | null>(null);
  const [confirming, setConfirming] = useState<number | null>(null);

  const ordered = [...projects].sort((a, b) => b.updatedAt - a.updatedAt);
  const create = (event: React.FormEvent) => {
    event.preventDefault();
    const trimmed = name.trim();
    if (!trimmed) return;
    onCreate(trimmed, withCurrent && photoCount > 0);
    setName("");
  };

  return (
    <div className="library" role="dialog" aria-label="项目库">
      <div className="library-head">
        <h2>项目库</h2>
        {activeId !== null && <button className="button secondary" onClick={onLeave}>关闭当前项目</button>}
        <button className="button secondary" onClick={onClose}>返回</button>
      </div>

      <form className="library-new" onSubmit={create}>
        <input aria-label="项目名称" placeholder="项目名称，例如：婚礼 2026-11" value={name} onChange={(event) => setName(event.target.value)} />
        {photoCount > 0 && activeId === null && (
          <label>
            <input type="checkbox" checked={withCurrent} onChange={(event) => setWithCurrent(event.target.checked)} />
            放入当前 {photoCount} 张图片
          </label>
        )}
        <button className="button" disabled={!name.trim()}>新建项目</button>
      </form>

      {ordered.length === 0 && <p className="library-empty">还没有项目。新建一个，把一组照片和它们的修图放在一起。</p>}
      <div className="library-grid">
        {ordered.map((project) => (
          <article key={project.id} className={`project ${project.id === activeId ? "current" : ""}`} data-project={project.name}>
            <button className="project-cover" onClick={() => onOpen(project)} aria-label={`打开 ${project.name}`}>
              {project.cover ? <img src={project.cover} alt="" /> : <span>空项目</span>}
            </button>
            {renaming?.id === project.id ? (
              <form
                className="project-rename"
                onSubmit={(event) => {
                  event.preventDefault();
                  if (renaming.name.trim()) onRename(project, renaming.name.trim());
                  setRenaming(null);
                }}
              >
                <input aria-label="新名称" autoFocus value={renaming.name} onChange={(event) => setRenaming({ id: project.id, name: event.target.value })} />
                <button className="button secondary">保存</button>
              </form>
            ) : (
              <h3>{project.name}</h3>
            )}
            <p>{project.photoIds.length} 张 · {new Date(project.updatedAt).toLocaleDateString()}</p>
            <div className="project-actions">
              <button className="button secondary" onClick={() => setRenaming({ id: project.id, name: project.name })}>重命名</button>
              {confirming === project.id ? (
                <button className="button danger" onClick={() => { setConfirming(null); onDelete(project); }}>确认删除</button>
              ) : (
                <button className="button secondary" onClick={() => setConfirming(project.id)}>删除</button>
              )}
            </div>
          </article>
        ))}
      </div>
      <p className="library-note">项目只记录照片和修图参数，删除项目不会动磁盘上的原图。</p>
    </div>
  );
}
