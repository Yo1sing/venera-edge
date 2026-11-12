// Source loading, validation and account state execute inside Rust's QuickJS worker.
globalThis.__sources = {
    newer(a,b) {
        const x=a.replace('-', '.').split('.'), y=b.replace('-', '.').split('.');
        for(let i=0;i<3;i++) { const l=Number(x[i]??0), r=Number(y[i]??0); if(!Number.isFinite(l)||!Number.isFinite(r)) throw Error('Invalid source version'); if(l!==r) return l>r; }
        if(x[3]===y[3]) return false;
        if(x[3]===undefined) return y[3]!=='hotfix';
        if(y[3]===undefined) return x[3]==='hotfix';
        return x[3]>y[3];
    },
    install(code) {
        const declaration=code.match(/^\s*class\s+([A-Za-z_$][\w$]*)\s+extends\s+ComicSource\b/m);
        if(!declaration) throw Error('Invalid comic source: missing ComicSource class');
        const source=(new Function(code+'\n;return new '+declaration[1]+'();'))();
        for(const field of ['name','key','version']) if(typeof source[field]!=='string'||!source[field]) throw Error(field+' is required');
        if(!/^[A-Za-z0-9_]+$/.test(source.key)) throw Error('Invalid source key');
        if(Object.prototype.hasOwnProperty.call(ComicSource.sources,source.key)) throw Error('key('+source.key+') already exists');
        if(source.minAppVersion && this.newer(source.minAppVersion,appVersion.split('-')[0])) throw Error('minAppVersion '+source.minAppVersion+' is required');
        if(source.idMatch) new RegExp(source.idMatch);
        if(source.account?.login) {
            const original=source.account.login;
            source.account.login=async function(account,password) {
                const result=await original.call(this,account,password);
                source.saveData('account',[account,password]);
                return result;
            };
        }
        const wrap=(object,name,requireLogin) => {
            if(typeof object?.[name]!=='function') return;
            const original=object[name];
            object[name]=async function(...args) {
                if(requireLogin&&!source.isLogged) throw Error('Not login');
                try {return await original.apply(this,args);}
                catch(error) {
                    if(!String(error).includes('Login expired')) throw error;
                    if(!await __sources.reLogin(source.key)) throw Error('Login expired and re-login failed');
                    return original.apply(this,args);
                }
            };
        };
        for(const name of Object.keys(source.favorites??{})) wrap(source.favorites,name,true);
        for(const name of ['sendComment','sendChapterComment']) wrap(source.comic,name,false);
        Object.defineProperty(ComicSource.sources,source.key,{value:source,writable:true,configurable:true,enumerable:true});
        return {name:source.name,key:source.key,version:source.version,url:source.url??''};
    },
    init(key) {
        const source=ComicSource.sources[key];
        if(typeof source?.init==='function') Promise.resolve().then(()=>source.init()).catch(e=>console.error(String(e)));
    },
    async reLogin(key) {
        const source=ComicSource.sources[key], account=source?.loadData('account');
        if(!Array.isArray(account)||!source.account?.login) return false;
        try {await source.account.login(...account);return true;}catch(error){console.error(String(error));return false;}
    },
};
